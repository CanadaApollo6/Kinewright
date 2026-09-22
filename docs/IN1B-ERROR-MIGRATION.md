# IN1b — The error migration

Status: contract revision 2 — promoted 2026-09-15, after one brief critique, one contract critique and two probes
Programme: [Roadmap and workflows](ROADMAP-AND-WORKFLOWS.md) "Investigator and harness programme", stage IN1, Part B
Discharges: [IN1 — Incidents, policy, and the colour case](IN1-INCIDENTS-AND-THE-COLOUR-CASE.md) **§10**, and its deferrals **D3**, **D4** and **D16**
Depends on: IN1 Part A, landed at `29fad67` and `d4ed8eb`
Scope: **closing the untyped error sink in the desktop application — every one of the 129 measured error paths, plus the three log-bypassing sinks of §5.7, becomes an `Incident` with a code, a policy class, a severity and a written recovery sentence, or it does not reach the person at all; the toolbar badge counts open problems rather than log lines; and the agent surface stops parsing its own rendered sentences back into codes.**

This companion exists because IN1 §10 is a sizing, not a contract: eleven prose rules, no numbered exit-gate clauses, no per-rule test, no probe markers and no files table. Nothing in §10 is dischargeable. IN1 §10 gains one pointer paragraph at this contract's promotion and its §13 D3/D4/D16 gain owner lines; D14 is re-deferred (§13 below). The roadmap sentence becomes *"IN1 lands in two parts on two contracts, IN1b discharging IN1 §10"* in the same commit (N0/Q1).

IN1b adds **no served MCP tool**, **no `Operation` variant** and **no capability**. The served quad stays **7 / 5 660 B / 3 510 B / 998 B** for the **seventeenth** consecutive measurement, asserted unmoved in all **three** pin sites (§6.4), and the registry sextuple stays **140 / 54 / 86 / 1 551 301 / 1 407 012 / 121 315**.

The words **must**, **must not** and **may** are normative. Rules are numbered inside their section so a critic can cite "§3 rule 12". Every code claim carries `file:line` at **`d4ed8eb`** and was re-measured 2026-09-15 by probe-2b and again by this revision. Revision 1's four `[probe-2b]` markers are all replaced by the measured value; the **one** number that remained unmeasured, written `[probe-2c]`, was measured against the implementation and §3.11 rule 43 carries it — **536 pairs, worst 1 351 B in core and 1 358 B in the agent's own loop, both under 2 048, with `IN1_INCIDENT_SERIALIZED_BYTES` unmoved at 819**. No marker remains.

---

## 0. Change log

### 0.1 Rulings carried from `target/review/in1b/orchestrator-notes.md`, by id

Every ruling of **N0** (Q1–Q13), **N1** (B1–B6), the **N1 addendum**, **N1.5** (1–13), **N2** (a–h) and **N2.5** (critic B1–B8, probe 1–12) is binding and is not relitigated. Each is carried here by its id with the section that discharges it.

- **N0/Q1 — companion contract.** This file, `docs/IN1B-ERROR-MIGRATION.md`; erratum ranges restart at core R1–R19, media R20–R39, app R40–R59, agent R60–R79. Header, §14.
- **N0/Q2 — 11 families**, confirmed by probe-1b P1 and probe-2b T6: the exhaustive match compiles over all 154 variants and Appendix A is an exact bijection with the enum. §3.1, Appendix A.
- **N0/Q3 — the hybrid code rule.** Every existing `code()`/`recovery_code()` string is reused verbatim; a family code is minted only where none exists; a label placeholder only where §3.2 rule 12's placeholder row permits it. §3.2.
- **N0/Q5, as amended by N1/B5 — `KinewrightApp::note_incident` is the single writer**; `ErrorLog::push` is module-private; `record_error` is deleted. IN1 §10 rule 6 is superseded and §5.3 says why. §5.3.
- **N0/Q6 — the badge reads `open_count()`**; the audit window keeps `entries.len()`. §5.4.
- **N0/Q7 — `IncidentSubject` grows**, each variant with its dedup axis stated; the forced breaks are budgeted, not worked around. §3.3, §3.8.
- **N0/Q8 — D14 is re-deferred now**, not cut later; nothing in Part B pre-builds for it. §1 non-goals, §13 D14.
- **N0/Q9 — the prefix stays; the three `strip_prefix` parsers are in scope** and sit second on the cut list; dropping the prefix is re-deferred to IN2. §6.2, §12, §13 D-B4.
- **N0/Q10 — `revert_available`, `card_action_outcome` generalised against the evidence, a badge-anchored Incidents panel**; placement stays untested. §5.5.
- **N0/Q11 — `get_incidents` unchanged; `verify_claimed_outcome` per code; `ResolveIncidentOutcome` unchanged; the served quad asserted unmoved as the seventeenth consecutive measurement in all three pin sites.** §6.
- **N0/Q12 — the exit gate**, with `cargo check -p kinewright-app` and the workspace gate before each commit. §9.
- **N0/Q13 — the population is 129**, stated once, in one form, with the counting rule. §2.
- **N0/additional — the Part A follow-up lands first.** It did: `d4ed8eb`. The census was re-run against it twice (probe-1b, probe-2b T6) and no count moved.
- **N1/B1 — the goal is re-scoped, not widened.** The agent crate's refusals are a distinct population, carried as the measured deferral **D-B1**. §1, §13 D-B1.
- **N1/B2 — every production irrefutable `let` is listed with its replacement**; `policy_recovery` becomes subject-generic. §3.7, §3.8.
- **N1/B3 — `policy_class(code, &IncidentEvidence)` and `probed() -> Option<&ColorDescription>`**, with all ten call sites enumerated (probe-2b T9 confirms all ten at the cited lines). §3.4, §3.5.
- **N1/B4 — `POLICY` gains a severity column**; every row states its value with a reason. §3.6.
- **N1/B5 — `note_incident` writes `status`, the audit line and the window flag**; a test asserts both. §5.3, §9 clause 6.
- **N1/B6 — every exit clause is pinned to a Part B figure** and none passes at `d4ed8eb`. §9.
- **N1 addendum — `Command::DoIfRevision` gains a correlation token** echoed on `Event::RevisionConflict` and `Event::DocumentChanged`. §4.
- **N1.5 §1 — `AgentError` is deleted from the typed-payload table.** §2.2.
- **N1.5 §2 — `CaptionPlanError` is in scope.** §2.2, §3.10.
- **N1.5 §3 — `MediaError` gains two typed passthrough variants.** §3.9.
- **N1.5 §4 — 17 labels, not 16**; one per-label test per label. §2.1, §7.
- **N1.5 §5 — both `load_error` producers write the literal `"Project"`.** §5.4 rule 25.
- **N1.5 §6 — seven irrefutable-let breaks.** §3.8.
- **N1.5 §7 — the migration budget is the measured ≈1 650 (band 1 160–2 190).** §12.
- **N1.5 §8, as amended by N2.5/probe 9 — `incident_family()`'s arms are grouped by family with `|` patterns**, and the one `#[allow(clippy::too_many_lines)]` is **required**, not conditional. §3.1 rule 3.
- **N1.5 §9 — Appendix A's corrections are accepted as the prober names them.** Appendix A.
- **N1.5 §10 — the parsers and the per-variant LutStore/RoomTone codes are one cut item**, second. §12.
- **N1.5 §11 — R5 is withdrawn.** §10 limit 3.
- **N1.5 §12, as amended by N2.5/probe 3–4 and 11 — the gate counts are spelled as greps with definitions excluded.** §5.3 rule 19.
- **N1.5 §13, as settled by N2.5/probe 2 — both size constants are measured**: 819 and 2 048, with no erratum to the `worst > CEILING / 2` assertion. §3.11, §10.
- **N2/a–h — the 67-code set, `edit_revision_conflict`, `look_incomplete`/`media_incomplete`, `unknown_source_white_point`, the matte passthrough variants, the three delegating producers, the `Project` unit variant and the empty `Informs` column** are all ratified and carried. §0.2, §3.
- **N2/`Option<CommandToken>` on `DoIfRevision`, none on `DoBatchIfRevision`** — carried, on the corrected reason. §4 rules 3–4.
- **N2.5/B1 — the thirteen colour bodies are one string**, carved out by name in §3.7, §5.6 and §9 clause 4.
- **N2.5/B2 — `ExportJob` and `Agent` are unit variants.** §3.3.
- **N2.5/B3 — `Chain(AudioChain)` replaces `Track` for the three Mixer rows**; `Track(TrackId)` survives on one measured row. §3.3.
- **N2.5/B4 — three log-bypassing sinks come in scope; the rest are D-B2/D-B3.** §1, §5.7, §13.
- **N2.5/B5 — `DoBatchIfRevision` keeps its conclusion on the true reason.** §4 rule 4.
- **N2.5/B6 — the errata list is completed.** §14.
- **N2.5/B7 — `Operation::incident_subject()`.** §3.3 rule 18, §7 item 4.
- **N2.5/B8 — evidence follows the code.** §3.4 rule 24.
- **N2.5/probe 1 — rule 13 amended, 56 reachable beside 67 declared.** §3.2 rule 13.
- **N2.5/probe 2 — the ceiling is 2 048, the fixture 819, the conditional struck.** §3.11 rule 43.
- **N2.5/probe 5 — `record_error` survives as an unchanged shim through steps 1–7.** §5.3 rule 20.
- **N2.5/probe 6–8 — §4's `Option` reason, `RouterConflict`'s lost field, `recovery.rs:1011`.** §4, §14.
- **N2.5/probe 10 — D-B1 is 277.** §13.
- **N2.5/probe 12 — the two clippy facts.** §14.

### 0.2 What revision 2 changed, by id

Every finding of `critic-contract.md` and `probe2b-report.md` that N2.5 accepted is folded. This is the index; the section named carries the change.

**Critic blocking.** **B1** → §3.7 rule 34, §5.6 rule 32, §9 clause 4, §12's never-cut list: the delegated set carries **16** distinct sentences, thirteen codes share `ColorSourceError::recovery_action()`'s one string, and the clause asserts what the test asserts. **B2** → §3.3 rule 18: `ExportJob` and `Agent` are unit variants, no new id type is minted, the `ExportJobId` collision with `export_queue.rs:46` disappears. **B3** → §3.3 rule 17, rule 19, §11 item 3: `Chain(AudioChain)` for the three Mixer rows; `Track(TrackId)` is declared because Appendix B row 124 constructs it. **B4** → §1's goal sentence, §5.7, Appendix B rows 28, 39 and 43, §13 D-B2/D-B3. **B5** → §4 rule 4. **B6** → §14's sixteen errata. **B7** → §3.3 rule 20. **B8** → §3.4 rule 24.

**Critic substantive.** **S1** → §3.5 rule 27 ("assigns" against "consults"). **S2** → §5.3 rule 19's four spelled greps. **S3** → §3.8's re-cited `app.rs:98`, `:4423`, `:4746`. **S4** → Appendix B rows 65, 116 and 125, all `Project`. **S5** → Appendix B row 9, `look_incomplete`/`Degrades`, consistent with row 3. **S6** → §3.2 rule 15 (the actor-stopped rule) and §5.6's re-read bodies. **S7** → §5.6's widened `agent_branch_rejected`, `source_edit_rejected` bodies and §3.10 rule 39's carve-out for the two id-exhaustion caption variants. **S8** → §3.2 rule 13 as amended, with §3.2 rule 14's delegation sub-table. **S9** → §5.1 rule 12 (`from_media_error` becomes total). **S10** → §4 rule 8. **S11** → §4 rule 7's eight arms, §12's token row, §14's app row. **S12** → §5.1 rule 11's step table, rebuilt and its reading stated. **S13** → §14 R6. **S14** → §14's `IN1b-` namespace. **S15** → §5.1 rule 14's stated behaviour change. **S16** → §13 D-B6 and D-B7 costs.

**Critic nits.** **N1** → §3.3 rule 18 (no id newtype is minted, so `model.rs`'s macro is not cited). **N2** → §3.6 rule 32's `incident.rs:1149`. **N3** → §14 R8's re-cite. **N4** → §9 clause 17's honest "fails by a word". **N5** → §7 item 17's stated mechanism. **N6** → §5.6 rule 32's note on the two template-sharing delegated sentences.

**Probe.** **1** → §3.2 rule 13. **2** → §3.11 rule 43, §9 clause 16, §10. **3, 4, 11** → §5.3 rule 19. **5** → §5.3 rule 20. **6** → §4 rule 3. **7** → §4 rule 6. **8** → §14's app row. **9** → §3.1 rule 3. **10** → §13 D-B1. **12** → §14's implementer notes.

### 0.3 Deviations this revision makes from the rulings, with evidence

**Three.**

- **D1 — N0/additional's line-budget band of 5 000–7 500 insertions is raised to 5 600–8 900.** Ratified as a deviation at N2 in the form 5 400–8 700; §12's table has since gained three rows (`Operation::incident_subject()`, the three §5.7 sinks) and replaced its token estimate with probe-2b T3's **measured** 172 insertions, so the table totals **4 392–6 992** before calibration and **5 600–8 900** after Part A's measured +28 %. The ruling's intent — say the number rather than discover it — is honoured; only the number moves, and it moves because the table moved.
- **D2 — N1/B4's default rule for placeholder severity is applied with a different default.** Ratified at N2. §3.6 rule 30 states the criterion the ruling was reaching for — *did the thing the person asked for happen?* — and derives each row from its site's own control flow at `d4ed8eb`. The outcome is **54 `Blocks`, 13 `Degrades`, 0 `Informs`** over the 67 declared rows, unmoved by revision 2's site corrections because severity is declared per code, not per site (N2/c).
- **D3 — `Operation::incident_subject()` also returns `Track(TrackId)`.** N2.5/B7 enumerates the accessor's returns as "`Asset`/`Clip`/`Chain` … and `Project` otherwise", written while B3 left `Track`'s declaration open. `Track(TrackId)` **is** declared (Appendix B row 124 constructs it at `timeline_ui.rs:3266`, where `RoomToneCaptureJob::track` is a `TrackId` in scope, `timeline_ui.rs:3315`), and five `Operation` variants address a track and nothing narrower — `AddTrack`, `RemoveTrack`, `SetTrackSyncLock`, `SetTrackMix`, `SetTrackAutomation` (`operation.rs`). Returning `Project` for those five would put a track refusal on the project's dedup axis while a room-tone refusal on the same track keeps its own, which is the inconsistency the axis exists to prevent. The accessor therefore covers every declared subject kind; §3.3 rule 18 states the precedence. **Erratum (A-R11, 2026-09-16):** **seven** `Operation` variants address a track and nothing narrower, not five — `AddTitle { track, … }` and `RippleInsertGap { track, … }` join the five D3 enumerates under §3.3 rule 20's precedence (`crates/kinewright-core/src/operation.rs:5166-5172`, `:5214-5216`). D3's list is a list of the variants D3 names, not a count of the accessor's `Track` arms, and §7 item 4's assertion is satisfied either way and is now asserted on values for all seven.

### 0.4 Changes from implementation and review

Every erratum the implementation and the two review passes per crate produced, by id, with the rule it amends. Each is written in full against that rule; this is the index. Ids live in this contract's own per-implementer ranges (core R1–R19, media R20–R39, app R40–R59, agent R60–R79) and are spelled `IN1b-A-R1` … in the reports; the `IN1b-R1a` … `IN1b-R16` namespace of §14 belongs to the errata this contract writes into the **IN1** contract and is a different list.

**A — core (15 written).**

- **A-R1** → §3.9 rule 37 — matte arms take `media_backend_unclassified`, not `None`.
- **A-R2** → §5.1 rule 12, Appendix B row 27 — `from_media_error` gains a fallback subject.
- **A-R3** → §3.1 rule 1 — `IncidentFamily` derives `Serialize` for the evidence.
- **A-R4** → §3.2 rule 11 — `field()` delegates for three, not four.
- **A-R5** → §14 row B — six media test assertions had to move.
- **A-R6** → §3.11 rule 43 — measured in core, moved by D.
- **A-R7** → §9 regression R-A — four more Part A tests rewritten.
- **A-R8** → §3.1 rule 3 — six `too_many_lines` allows stand, not one.
- **A-R9** → §2.1 rule 4 — row 27 ceased; population 128, Media 7.
- **A-R10** → §5.5 rule 30 — the headline test must discriminate against the body.
- **A-R11** → §0.3 D3 — seven track-addressed `Operation` variants, not five.
- **A-R12** → §3.9 rule 36, §14 row B — `MediaError::Store` lands; `Some` for 9 of 14.
- **A-R13** → §3.3 rules 17, 20, 21, §3.11 rule 42 — an eighth subject, `LutAsset`; 536 pairs.
- **A-R14** → §14 row B — `LutParseError` converted; `ColorPipelineError` deferred, named.
- **A-R15** → §4 rules 2 and 5 — `Event::OpRejected` gains the correlation token.

**C — app (13 written, 1 withdrawn).**

- **C-R40** → §5.7 rule 34 — `restore_status`'s `Err` observation is boxed.
- **C-R41** → Appendix B rows 117–121, 128, 129 — seven `Clip` rows take `Project`.
- **C-R42** → **withdrawn**: stage A addendum 2 declares `IncidentSubject::LutAsset(LutAssetId)`, so the seven `Look` rows no longer need `Project`; C-R52 narrows what survives.
- **C-R43** → §2.2 rule 8 — `RelinkRevisionConflict::message` is deleted, not kept.
- **C-R44** → §5.4 rule 27 — the two error-category constants become incident-code constants.
- **C-R45** → §3.1 rule 3 — the app crate adds two clippy allows.
- **C-R46** → §7 item 16, §9 clause 8 — four per-label tests prove the declaration.
- **C-R47** → §5.2 rule 16 — four `note_*` shorthands sit beside `note_incident`.
- **C-R48** → Appendix B rows 99, 104 — `MediaError` evidence carries the incident's code.
- **C-R49** → §5.4 rule 25 — badge visibility is decided separately from its number.
- **C-R50** → §4 rule 9 — a landing token is the first condition.
- **C-R51** → Appendix B row 26 — the router's own refused send opens nothing.
- **C-R52** → Appendix B rows 96, 99–104 — five `Look` rows take `LutAsset`, two `Project`.
- **C-R53** → §4 rule 7 — a token-matching rejection drops the outstanding send.
- **C-R54** → §4 rule 7, Appendix B rows 24 and 26 — the two skips apply to `Auto` sends only.

**D — agent (7 written, 1 withdrawn).**

- **D-R60** → §6.1 rule 2 — `verify_source_colour_outcome` does not take the incident.
- **D-R61** → **withdrawn** at N4: `MediaError::Store` (A-R12) landed the typed store passthrough, so cut item 2 is not cut, `media_refusal_code`'s leading-token fallback is deleted, and §9 clause 14 is discharged for both code-consuming sites.
- **D-R62** → §6.4 rule 8, §9 regression R-A — the counter-carrying pin site is renamed.
- **D-R63** → §9 P1 — `cargo fmt --all` is not a per-implementer command.
- **D-R64** → §3.11 rule 42 — `get_incidents`' description is unchanged; the pins win.
- **D-R65** → §6.1 rule 2 — a probe-less colour claim now records, not refuses.
- **D-R66** → §6.2 rule 4 — one of the three parsers reads the payload only.
- **D-R67** → §6.2 rule 4 — `LutParseError` typed; the parse code is served again.

**Observations for later slices.** Five things the implementation and the reviews measured that no erratum fixes, recorded here so the next slice finds them rather than rediscovers them. **(1) Two family bodies mis-advise a measured case, and both belong to IN3** with the thirteen colour bodies of IN1 §13 D8: `operation_unrepresentable` tells the person *"Choose a different frame or a different duration"* for `IncorrectDocumentDuration`, which Appendix A's own footnote says no caller-supplied value can help, and `operation_relink` tells them to *"Relink from the media panel"* for `SourceFingerprintIncomplete`, `InvalidSourceFingerprintHash` and `InvalidSourceFingerprintByteLength`, all three of which are also reachable from `Operation::AddAsset`'s validation, where nothing is being relinked and the fix is to re-import (review-core-2 N4, N5). §5.6 pins the text and §9 clause 4 asserts the pinned text, so neither is a Part B defect. **(2) `ColorPipelineError` is not converted**, and the reason is three measured facts rather than an omission: it declares no `code()` or `as_str()` and only three of its variants render a leading `snake_case` token, it has no `From<ColorPipelineError> for MediaError` to redirect — it reaches `MediaError` only through ad-hoc `Backend(format!(…))` sites in `render.rs` — and it reaches neither `lut_store_error_result` nor `room_tone_store_error_result`, so no served result regressed. Giving it codes is a slice of its own (erratum A-R14's deferred half). **(3) A `recovery_code()` string is a value to serve, never a variant identity.** Four enums in `kinewright-media` mint code strings independently — `LutStoreErrorCode`, `RoomToneStoreErrorCode`, `LutParseErrorCode` and `ColorPipelineError` — and two of them render `missing_lut_asset`, so the obvious fix for D-R67 (matching a leading token against the eleven `LutStoreErrorCode` strings) would mis-attribute a render refusal to the store. Nothing in the workspace branches on a code string and nothing should; `in1b_a_served_code_string_is_not_a_variant_identity` (`crates/kinewright-media/src/lut.rs:952`) pins both halves. **(4) §5.5 rule 30 declares no headline *text* table** (review-app-1 S4, ruled at N5.5): the review brief's text-table check has no referent, and the invariants — 70 rows, pairwise distinct, exhaustive in both positions, never equal to `explain_body` — are the check. They pass; no code changed. **(5) A test mirror of production logic pins nothing.** The final fresh-eyes review measured it: C-R50's landing push, C-R51's conflict skip and C-R53's rejection drop each lived in `poll_background`'s drain **and** in parallel source in the test mirror `in1_drain_core`, and deleting any one of the three production copies left the 553-test app suite green — including the landing push, without which no router auto-apply ever resolves in the real application. Making the mirror *match* production, which is what ruling N5.7 S2 asked for, did not remove the second copy; the fix was to give the decisions **one owner**, `KinewrightApp::note_core_event_for_router` (`crates/kinewright-app/src/app.rs:1314`), called by both the drain and the mirror, and to convert two router tests to tick the real `poll_background` through `in1_poll_production_until` (`app.rs:4724`). The same three mutations then failed 3, 1 and 3 tests. The rule this leaves for later slices: a decision that a test mirror re-implements is untested, however faithfully the mirror copies it — the mirror must call the decision, not repeat it.

---

## 1. Goal, and what Part B does not deliver

Part A proved one path: one typed source-colour failure becomes an `Incident` with a declared `PolicyClass`, a recovery the router applies itself, a card that says what Kinewright did, and two capabilities an agent reads it through. Part B makes that the only path through the desktop application:

> **Every error path that reaches the person through the app's log, status bar or chat transcript reaches them as an incident, or not at all.** (N1/B1, as narrowed by N2.5/B4)

The narrowing is deliberate and measured. The app also speaks to a person through panel-local labels, two modal dialogs and disabled-control tooltips — states the incident model does not yet describe, inventoried and deferred by name in §13 D-B2 and D-B3. The sentence claims the three surfaces the gate proves and no more.

At `d4ed8eb` the application still has **129** error paths that flatten a typed failure into a `String` and drop it into a scrolling window behind a toolbar badge — the shape that produced the roadmap's finding that "a person without colour knowledge had no way to find it" (`docs/ROADMAP-AND-WORKFLOWS.md:818-828`) — and **three** further sinks that reach the same person through the status bar and the chat transcript without touching the log at all (§5.7). After Part B a message reaches a person through any of the three surfaces with a code, a class, a severity and a sentence naming what must change, or it does not reach them.

**What Part B makes true.**

1. `KinewrightApp::record_error` does not exist, and no symbol in the app crate writes to `ErrorLog` except one `pub(crate) fn note_incident` whose input is an `&Incident` (§5.3).
2. `POLICY` is exhaustive over **67** codes, each with a class, a predicate, a severity and an `Explain` body (§3.2, §3.6, §5.6).
3. `OpError`'s 154 variants each answer one question — *what must change for the same request to succeed?* — through an exhaustive `incident_family()` (§3.1).
4. The toolbar badge counts problems currently unresolved, not lines ever written (§5.4).
5. The router matches a `RevisionConflict` to the send it refused **by identity**, not by `(project, expected)` (§4).
6. The agent's three `strip_prefix("media backend error: ")` parsers are replaced by the typed `recovery_code()` they were reconstructing by hand (§6.2).
7. The two chat-panel revision conflicts and the crash-recovery restore failure stop being bare status strings (§5.7).

**Part B does not deliver.** The inventory is normative; each item is a thing a reader might reasonably expect.

1. **No typing of the agent crate's own refusals.** `error_text(` and `error_structured(` in `crates/kinewright-agent/src/server.rs` are agent-facing, are a different population of **277** call sites, and are a measured deferral (§13 D-B1). The sink gate's four counts are greps over the app crate and **do not claim them**; §5.3 rule 21 says so beside the gate.
2. **No typing of the panel-local sinks.** Nine `Unavailable(String)` labels, two `recovery.rs` modals and the LUT-store tooltips keep their strings (§13 D-B2), and four `Result<_, String>` seams keep theirs (§13 D-B3). §1's goal sentence excludes them by name rather than by silence.
3. **No multi-project incident attribution.** IN1 §13 D14 is re-deferred with a new owner and nothing here pre-builds for it (§13 D14).
4. **No dropping of the `media backend error: ` prefix.** IN1 §9 clause 11's 750 B template and the two pinned literals do not move (§13 D-B4).
5. **No model, no investigator session, no `IncidentState::Investigating`** — IN2 (IN1 §13 D5).
6. **No persistence of incidents and no cross-process timestamp** — IN2 (IN1 §13 D1/D2).
7. **No typed broker payload and no `Approved`/`Rejected` outcomes** — IN2 (IN1 §13 D6).
8. **No `AskFirst` row and no `RecoveryKind::Relink`/`Transcode`.** Part B declares an `AskFirst` row only if a recovery exists to gate; none does (IN1 §13 D7).
9. **No `RecoveryKind::Operation` recovery for any new code.** Every Part B code is `Explain`; the three Part A `AutoApply` rows are the only rows that apply anything (§3.6 rule 28).
10. **No new `Operation` variant, no served-tool growth, no new capability.** Part B is a typing slice; if it needs a new operation it has found a different slice.
11. **No live `KinewrightApp` harness** (IN1 §13 D12) and **no tested card placement** (IN1 §11.1 limit 2).
12. **No widening of either managed-colour allowlist** (IN1 §13 D13).
13. **No per-code colour body.** The thirteen `ColorSourceError` codes keep the one shared sentence `ColorSourceError::recovery_action()` returns (`color.rs:328-332`). That is IN1 §13 D8's item and it is owned by IN3; §9 clause 4 names the thirteen rather than asserting a distinctness the tree cannot satisfy (N2.5/B1).

**IN1b is complete when** a person who does not know what a LUT store root, a delivery variant or a source fingerprint is can hit any of the failures the application still has on its log, its status bar or its chat transcript, read one sentence that names the thing they must change, see a badge that counts only the problems still unsolved — and when the agent in the chat panel can read that same failure, with its code, its field, its class, its severity and its recovery, in one call, at the same revision, for zero additional served bytes.

---

## 2. The population

### 2.1 The 129 paths, measured once

1. **The population, written once in this form and used with these figures everywhere.** At `d4ed8eb` the app crate has **124** `record_error(` call sites carrying a literal source label over **16** literal label spellings, **plus 2** dynamic-source sites, **plus 3** direct `ErrorLog::push` bypasses — **129** error paths. A **seventeenth** label, `"Timeline"`, is reachable only through one of the dynamic sites (rule 3). Re-measured twice: probe-1b's `census.sh`, and probe-2b's `census2.py`, which fixes that script's label-line off-by-one and reproduces the same 129 sites and the same 17 labels.

2. **The counting rule, normative.** A literal `record_error(` site counts as unmigrated until the symbol is gone; a site made *conditional* is **not** migrated. IN1 §5.2 rule 6's `else` arm at `app.rs:1637` is therefore in the population, and so are Part A's own three sites: the two `Operations` sends at `app.rs:1088` and `:1251` and the `Incident` audit line at `app.rs:1074`. The definition of `record_error` itself (`crates/kinewright-app/src/error_ui.rs:64`) is not an error path, is not in the 129, and is **excluded from the gate's grep** (§5.3 rule 19).

| # | source label | sites | where the sites are | Part B note |
| ---: | --- | ---: | --- | --- |
| 1 | Operations | 34 | `app.rs` ×11, `keys.rs` ×8, `timeline_ui.rs` ×8, `media_bin.rs` ×5, `inspector_ui.rs` ×1, `media_workflow.rs` ×1 | 3 typed (`OpError`, `BatchError`, revision pair); 31 string-only, of which 11 are actor-stopped (§3.2 rule 15) |
| 2 | Look | 16 (+1 bypass = 17) | `media_workflow.rs` ×11, `app.rs` ×3, `inspector_ui.rs` ×1, `look_browser_ui.rs` ×1, bypass `app.rs:760` | 2 carry a `MediaError` wrapping a `LutStoreErrorCode`; 2 are actor-stopped |
| 3 | Export | 12 | `export_ui.rs` ×12 | 3 `DeliveryVariantError`, 1 `MediaError`, 3 typed report summaries |
| 4 | Source monitor | 11 | `preview_ui.rs` ×6, `media_workflow.rs` ×4, `app.rs` ×1 | 1 `SourceEditRejection` |
| 5 | Relink | 8 | `media_workflow.rs` ×8 | 3 `RelinkRevisionConflict`, 2 `RelinkRejection`, 1 actor-stopped |
| 6 | Agent branch | 7 | `chat_ui.rs` ×7 | 2 `BatchError`, 3 `BranchError` |
| 7 | Transcript edit | 7 | `transcript_edit.rs` ×7 | all string-only |
| 8 | Media | 6 (+2 bypasses = 8) | `app.rs` ×3, `media_workflow.rs` ×1, `timeline_ui.rs` ×1, `transport.rs` ×1, bypasses `app.rs:775`, `:1377` | `app.rs:1637` is IN1 §5.2 rule 6's `else` arm and disappears (§5.1 rule 12) |
| 9 | Agent | 5 | `chat_ui.rs` ×5 | all string-only; `AgentError` is **not** referenced in the app crate |
| 10 | Recording | 5 | `recording.rs` ×5 | all string-only |
| 11 | Project | 4 | `app.rs` ×4 | 1 `ProjectSaveError` |
| 12 | Mixer | 3 | `app.rs` ×3 | all string-only, all `AudioChain`-scoped (§3.3 rule 17) |
| 13 | Captions | 3 | `captions.rs` ×2, `export_ui.rs` ×1 | 1 `CaptionPlanError` once `captions.rs:77` is re-typed |
| 14 | Branch preview | 1 | `chat_ui.rs:797` | `MediaError` |
| 15 | Media cache | 1 | `media_workflow.rs:1277` | |
| 16 | Incident | 1 | `app.rs:1074` | Part A's own audit line; becomes `note_incident` (§5.3) |
| 17 | **Timeline** | 0 literal | reached only through `inspector_ui.rs:793` | `ROOM_TONE_ERROR_CATEGORY` (`timeline_ui.rs:2907`) |
| — | *(dynamic)* | 2 | `app.rs:470`, `inspector_ui.rs:793` | §5.4 rule 27 |
| — | *(bypass)* | 3 | `app.rs:760`, `:775`, `:1377` | §5.1 rule 14 |
| | **total** | **129** | | |

3. **The seventeenth label is reachable and untested.** `inspector_ui.rs:793` passes `edits.error_category()` (`inspector_ui.rs:238`), which resolves to exactly two values: `LOOK_ERROR_CATEGORY = "Look"` (`inspector_ui.rs:37`) and `ROOM_TONE_ERROR_CATEGORY = "Timeline"` (`timeline_ui.rs:2907`, set at `timeline_ui.rs:3133`, `:3275` and in a test at `:5397`). `"Timeline"` never appears as a literal at a `record_error(` site, so a per-label test count of 16 would be short by one. §7 requires **17**.

4. **Per-label totals under the sink gate** are Look **17** and Media **7**, because the three bypasses carry the labels `"Look"` (`app.rs:760`) and `"Media"` (`app.rs:775`, `:1377`). Confirmed by probe-2b T6. **Erratum (A-R9, 2026-09-16):** Media reads **7**, not 8, and the population implementer C migrates is **128**, not 129: making `from_media_error` total (A-R1, A-R2) left Appendix B row 27's site (`crates/kinewright-app/src/app.rs:1646-1661`) with no `else` arm to keep, so its `record_error("Media", …)` call ceased to exist in stage A rather than being migrated. §2.1 rule 2's counting rule is not strained — the site was not made conditional, §5.1 rule 12 says it disappears — and the person-visible message moved with it, from `Playback error: {error}` to the incident headline through `note_incident`.

5. **Three further sinks are in scope and carry no label at all.** They never call `record_error` and never touch `ErrorLog`, so they are not in the 129 and not in any per-label count; they are Appendix B rows **28**, **39** and **43** and §5.7 owns them. Appendix B therefore has **132** rows: the 129 measured paths plus these three.

### 2.2 The typed payloads, and where each enum lives

6. The enums the 129 paths format, corrected per N1.5 §1–§3. An enum carrying a `code()` today **must** have that string reused verbatim (§3.2 rule 11).

| enum | variants | `code()` today | declared in | reached from |
| --- | ---: | :-: | --- | --- |
| `ColorSourceError` | 13 | **yes**, `color.rs:245` | core | Media, Source monitor, Export |
| `MediaError` | **11** | **`recovery_code()`**, `media.rs:1826`, `Some` for **6 of 11** | core | Media, Look, Export, Transcript edit, Branch preview, Media cache |
| `DeliveryColorError` | 4 | **yes**, `delivery.rs:784` | core | Export, via `MediaError::DeliveryColor` |
| `DeliveryVerificationError` | 5 | **yes**, `delivery.rs:913` | core | **not reachable from the 129** — §3.2 rule 13, §13 D-B3 |
| `ColorQcError` | 6 | **yes**, `color_qc.rs:719` | core | **not reachable from the 129** — §3.2 rule 13, §13 D-B3 |
| **`MatteProofError`** | **5** | **yes**, `media.rs:589` | core | flattened by `From` at `media.rs:600` into `MediaError::Backend` — §3.9 |
| **`MatteCoverageError`** | **5** | **yes**, `media.rs:648` | core | flattened by `From` at `media.rs:659` — §3.9 |
| **`CaptionPlanError`** | **6** | no | core, `captions.rs:68` | Captions, flattened at `crates/kinewright-app/src/captions.rs:74` — §3.10 |
| `LutStoreErrorCode` | 11 | **yes**, `lut_store.rs:135` | **media** | Look — flattened to `String` before the site (cut item 2) |
| `RoomToneStoreErrorCode` | 10 | **yes**, `room_tone_store.rs:188` | **media** | Recording — flattened to `String` (cut item 2) |
| `OpError` | **154** | no | core, `operation.rs:494-1060` | Operations |
| `BatchError` | 2 | no | core, `operation.rs:1063` | Operations, Agent branch |
| `DeliveryVariantError` | 3 | no | core, `delivery.rs:343` | Export |
| `TimeMappingError` | 7 | no | core, `time.rs:148-230` | Operations, behind `OpError::TimeMapping` |
| `BranchError` | 5 | no | **agent**, `branch.rs:47` | Agent branch |
| `SourceEditRejection` | 11 | no | **app**, `media_workflow.rs:116` | Source monitor |
| `RelinkRejection` | 2 | no | **app**, `media_workflow.rs:324` | Relink |
| `RelinkRevisionConflict` | struct | no | **app**, `media_workflow.rs:290` | Relink |
| `ProjectSaveError` | 2 | no | **app**, `project.rs:71` | Project |

7. **`AgentError` earns no code and its §2 row is deleted** (N1.5 §1). `grep -rn AgentError crates/kinewright-app/src` returns nothing at `d4ed8eb`: `AgentEvent::Error(String)` is a `String` (`chat_ui.rs:849` pushes `error.clone()` as a `ChatEntry::Text`) and the other four `Agent` sites are `format!`. The label takes a placeholder.

8. **Four of the migration's typed enums live outside core**, and Part B **must not** move any of them there. IN1 §2.1 rule 2 already permits it — *core owns the code and the policy class; core does not own the code's trigger*. The four are `BranchError` (agent crate) and `SourceEditRejection`, `RelinkRejection`/`RelinkRevisionConflict` and `ProjectSaveError` (app crate). Each gains an inherent `fn incident_code(&self) -> IncidentCode` in **its own** crate, reading the core enum; the code and the class stay core's. **Erratum (C-R43, 2026-09-16):** `RelinkRevisionConflict::message` is **deleted** rather than kept — its only three callers were Appendix B rows 85, 89 and 91, all migrated, and a `&'static`-shaped sentence with no production caller is `dead_code` under `-D warnings`. Its one test's two assertions move onto the incident, where the same two revisions reach the person as `allowed` and `observed` (`crates/kinewright-app/src/media_workflow.rs:2760`); the project name the sentence interpolated is dropped, because an incident is stated against its subject and the incident log is per-project by construction (IN1 §5.1 rule 1).

9. **Thirty-one existing `code()` strings are unreachable typed from the 129 paths** and the hybrid rule does not pick them up: `LutStoreErrorCode` 11 and `RoomToneStoreErrorCode` 10 are flattened to `String` before their sites; `MatteProofError` 5 and `MatteCoverageError` 5 are flattened by their `From` impls. §3.9 and cut item 2 are the two halves of that fact.

---

## 3. Core

### 3.1 `IncidentFamily` and `OpError::incident_family()`

1. **`IncidentFamily` is a core enum with exactly 11 variants**, declared in `crates/kinewright-core/src/operation.rs` beside `OpError` so the match cannot drift from the declaration it covers:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum IncidentFamily {
    Bounds, Malformed, Duplicate, Placement, Missing,
    Structure, Relink, Unrepresentable, UnknownName, Internal, ColorPolicy,
}
```

   Each family answers one question — *what must change for the same request to succeed?* — and each has a distinct recovery sentence, written in §5.6. Probe-1b P1 confirmed no family has an empty membership and no two share a sentence. **Erratum (A-R3, 2026-09-16):** the derive list gains `Serialize` with `#[serde(rename_all = "snake_case")]`, shown above (`crates/kinewright-core/src/operation.rs:4916`), because §3.4 rule 23 puts `family: IncidentFamily` inside `IncidentEvidence`, which derives `Serialize`. The rendering is the short name (`"bounds"`) and not `code()`, so the family and the incident's own `code` do not say the same word twice.

2. `pub const fn OpError::incident_family(&self) -> IncidentFamily` **must** be an exhaustive `match` over all **154** variants with **no wildcard arm**, so a new `OpError` variant breaks the build and forces a family decision. Appendix A is that match, normatively, and is an exact bijection with the enum at `d4ed8eb` (probe-1b P1 and probe-2b T6, independently: 154 rows, 0 duplicates, 0 omissions, every family count equal to its own list length).

3. **The arms are grouped by family with `|` patterns, and exactly one `#[allow(clippy::too_many_lines)]` sits on the function.** Probe-2b T8 built the deliverable and measured both halves. Grouping removes `clippy::match_same_arms` honestly — **11** firings become **0**. It saves **no lines**: rustfmt puts every `|` alternative on its own line, so the grouped body is the same **156** lines as the ungrouped one and trips `clippy::too_many_lines` at **156/100** either way. N1.5 §8's conditional therefore fires, and probe-1b's "grouping saves ~140 lines" estimate is withdrawn. One `#[allow(clippy::too_many_lines)]` on `incident_family()` and **no other allow anywhere** makes `cargo clippy -p kinewright-core --all-targets -- -D warnings` clean; **no blanket allow may be added**. `.github/workflows/ci.yml:49` runs that command over the workspace, so the contract and the house gate agree in writing. **Erratum (A-R8, 2026-09-16):** "no other allow anywhere" is falsified by two functions this contract itself adds — `Operation::incident_subject()`, 57 arms over 106 lines (`crates/kinewright-core/src/operation.rs:5163`), and `explain_body()`, 67 arms over 126 lines (`crates/kinewright-core/src/incident.rs:2000`) — so **six** `#[allow(clippy::too_many_lines)]` stand in the two core files, five beyond the one this rule mandates, each carrying a written reason line. No blanket allow was added and no other lint is allowed anywhere in the slice. **Erratum (C-R45, 2026-09-16):** the app crate adds two allows of its own, `clippy::unused_self` on `RelinkRevisionConflict::incident_code` (`crates/kinewright-app/src/media_workflow.rs:342`, a struct with no variants to match on) and `clippy::too_many_lines` on `incident_headline` (`crates/kinewright-app/src/incident_ui.rs:136`), each with a written reason; `app.rs:2976`'s `unused_self` is pre-existing at `b99c328` and is not this slice's.

4. **`OpError::TimeMapping` delegates.** `#[error(transparent)] TimeMapping(#[from] TimeMappingError)` (`operation.rs:1059`) carries **seven** sub-variants (`time.rs:148-230`), of which exactly one is internal. The arm **must** be `Self::TimeMapping(inner) => inner.incident_family()`, against a second `const fn TimeMappingError::incident_family(&self) -> IncidentFamily` in `crates/kinewright-core/src/time.rs`:

| `TimeMappingError` | family | why |
| --- | --- | --- |
| `InvalidRate { numerator, denominator }` | `Malformed` | supply a rate with a positive numerator and denominator |
| `NegativeFrames(TimeCode)` | `Bounds` | the frame count must not be negative |
| `Overflow` | `Internal` | the only one nobody can act on |
| `InvalidRange { start, end }` | `Malformed` | supply a non-empty, non-negative source range |
| `InexactDuration { … }` | `Unrepresentable` | no source range maps to exactly that many project frames |
| `NoCoveringSourceRange { … }` | `Unrepresentable` | no covering range exists |
| `SourceTooShortToCover { … }` | `Bounds` | the source is shorter than the requested cover |

   Without the delegation six person-fixable failures are told "nothing you can do — report it", which is the failure the whole slice exists to remove.

5. **`IncidentFamily::code(self) -> &'static str`** returns the family's stable code string: `operation_bounds`, `operation_malformed`, `operation_duplicate`, `operation_placement`, `operation_missing`, `operation_structure`, `operation_relink`, `operation_unrepresentable`, `operation_unknown_name`, `operation_internal`, `operation_color_policy`.

6. **The code producer is separate from the family map**, because exactly two variants earn a per-variant code:

```rust
#[must_use]
pub const fn OpError::incident_code(&self) -> IncidentCode {
    match self {
        // Minted per-variant (N1.5 §9): an allowlist refusal whose recovery
        // is the LUT store's own import, not "supply a different field".
        Self::InvalidLutAssetHash { .. } | Self::InvalidLutAssetMetadata { .. } =>
            IncidentCode::LutAssetPolicy,
        other => IncidentCode::Operation(other.incident_family()),
    }
}
```

   `incident_family()` stays **total** and returns `Malformed` for both, so Appendix A remains a partition of 154; the code is the override. A core test **must** assert that `incident_code()` and `incident_family()` agree for all 154 variants except those two.

7. **`ColorPolicy` survives as a routing hint, and the contract says so rather than claiming a distinct recovery.** Probe-1b P1 is right that it is the one family grouped by subject domain: its remaining three members share the incident card IN1 already ships, and that shared card *is* the recovery. `ColorConfidenceOutOfRange` leaves it for `Bounds`, because its message — `"color confidence is {actual}, outside the inclusive range 0..=10000 basis points"` — is a `Bounds` message word for word.

8. **Two `Bounds` members do not interpolate their bound.** `TrackAutomationOutOfRange` (`"…is outside its inclusive range"`) and `SplitOutsideClip` (`"split at project frame {at} is outside clip {clip}"`) name no range, so `Bounds`'s written body (§5.6) **must not** promise the person a range the card does not show. The body says "the message names the value you gave and, where it can, the range it must be in". Widening the two `#[error]` strings is permitted and is **not** required.

9. **Three `OpError` variants keep an untyped `reason: String` and Part B does not type them.** `InvalidTrackAutomation` (`operation.rs:982`), `InvalidClipGainEnvelope` (`:979`) and `InvalidEffectAutomation` (`:924`) are all built by `curve.validate().map_err(|error| … reason: error.to_string())`. Their family is `Malformed` and their card carries the stringified inner error inside `observed`. This is named here so nobody discovers it as a bug; promoting the `validate()` error to a typed field is **D-B5** (§13).

### 3.2 `IncidentCode` grows to 67

10. `IncidentCode` grows from one variant to the shape below. It keeps its hand-written `Serialize` emitting `self.code()` and nothing else (IN1 §2.2 rule 5), keeps `#[non_exhaustive]` off (IN1 §2.2 rule 10), and still has no catch-all, no `Unclassified` and no `Other(String)`:

```rust
pub enum IncidentCode {
    SourceColor(SourceColorIncident),     // 13 (§0.2/d adds the thirteenth)
    Media(MediaIncident),                 //  2: unsupported_decoder_format, media_backend_unclassified
    DeliveryColor(DeliveryColorIncident), //  4
    DeliveryVerification(DeliveryVerificationIncident), // 5
    ColorQc(ColorQcIncident),             //  6
    Operation(IncidentFamily),            // 11
    LutAssetPolicy,                       //  1  (§3.1 rule 6)
    EditRevisionConflict,                 //  1  (§0.2/b)
    Rejection(RejectionIncident),         //  7: edit_plan, delivery_variant, agent_branch,
                                          //     source_edit, relink, project_save, caption_plan
    Label(LabelIncident),                 // 17: 15 placeholders + look_incomplete + media_incomplete
}
```

11. **`code()` and `field()` delegate; no string is restated.** `IncidentCode::code()` **must** return the existing accessor's `&'static str` for every code that has one — `ColorSourceError::code()` (`color.rs:245`), `MediaError::recovery_code()` (`media.rs:1826`), `DeliveryColorError::code()` (`delivery.rs:784`), `DeliveryVerificationError::code()` (`delivery.rs:913`), `ColorQcError::code()` (`color_qc.rs:719`) — and the minted literal otherwise. `field()` delegates the same way; the four typed enums above all carry a `field()` accessor already. A core test **must** assert accessor-against-accessor over every delegating code, in the shape IN1 §2.2 rule 8 already uses. **Erratum (A-R4, 2026-09-16):** `field()` delegates for **three** of the four typed enums, not all four. `DeliveryColorError::field()` (`crates/kinewright-core/src/delivery.rs:800`) returns a `&str` borrowed from `&self`, because `UnsupportedField(mismatch)` answers `mismatch.field.as_str()` — a per-instance `String` — and `IncidentCode::field()` must return a `&'static str`; `DeliveryColorIncident`'s three static arms are therefore written out and `UnsupportedField` answers `"delivery_color"`, the name of the check rather than of the one field that failed (`crates/kinewright-core/src/incident.rs:290`), with the accessor-against-accessor test asserting the three.

12. **The measured set is 67 codes**, re-measured against this table by probe-2b T1, which generated all 67 rows mechanically and compiled them. **Erratum (E-B9, 2026-09-22):** the measured set becomes **74** codes — 67 + Part B's 7 §5 labels ([IN2B](IN2B-INCIDENT-PERSISTENCE.md) §0.5 E-B9, §5):

| origin | codes | detail |
| --- | ---: | --- |
| existing `code()` strings | **29** | 12 Part A rows + `unknown_source_white_point` + `unsupported_decoder_format` + 4 `DeliveryColorError` + 5 `DeliveryVerificationError` + 6 `ColorQcError` |
| family codes | **19** | 11 `OpError` families + `media_backend_unclassified`, `edit_plan_rejected`, `delivery_variant_rejected`, `agent_branch_rejected`, `source_edit_rejected`, `relink_rejected`, `project_save_failed`, `caption_plan_rejected` |
| label placeholders | **15** | one per string-only source label, including `timeline_unclassified` |
| ruled, minted per-variant | **1** | `lut_asset_policy` (N1.5 §9) |
| drafted, minted | **3** | `edit_revision_conflict`, `look_incomplete`, `media_incomplete` (§0.2/b, §0.2/c) |
| | **67** | probe-1b measured **63** of these; the four additions are §0.1 and §0.2 |

   Class split: **3 `AutoApply`** (Part A's three `Rec709Compatible` rows, unchanged), **0 `AskFirst`**, **64 `Explain`**. Severity split: **54 `Blocks`**, **13 `Degrades`**, **0 `Informs`**. `POLICY` goes from **12 rows to 67**, of which **55 are new `Explain` rows** — which is the true size of the migration; §5.6's writing task is the **39** bodies with no accessor behind them. Every one of these splits reproduced exactly under probe-2b T1.

13. **Reachability, stated as the two numbers it is.** **56** of the 67 codes are reachable from one of the 129 paths; **11** are not. A code **must not** be declared with no site in Appendix B **unless** an existing `code()` accessor already ships it and `IncidentCode::code()`'s delegation to that accessor would otherwise be non-total; §13 then **must** name the seam that stops it. Exactly eleven codes are declared under that exemption — the five `DeliveryVerificationError` and six `ColorQcError` rows — and their two seams are **D-B3**. Dropping them would make the delegation partial and would put `MediaError::DeliveryVerification`/`ColorQc` back into `media_backend_unclassified`, losing exactly the typed code §10 exists to keep. This is the same treatment §0.2/e gives the matte errors, which take the opposite branch because no `IncidentCode` is declared for them at all. **Erratum (E-B9, 2026-09-22):** 56/11 becomes **74/0** — the 11 D-B3 codes are reachable through the §6 seams and the 7 new codes through their §5 notes ([IN2B](IN2B-INCIDENT-PERSISTENCE.md) §0.5 E-B9, §6).

14. **The three delegating rows expand, so rule 13 is checkable.** Appendix B rows 27, 46 and 61 read `MediaError::recovery_code()`; the codes they can resolve to are:

| row | site | codes reachable through the accessor |
| ---: | --- | --- |
| 27 | `app.rs:1637` | the 13 `SourceColor` codes (incl. `unknown_source_white_point`, §0.2/d), `unsupported_decoder_format`, `media_backend_unclassified` |
| 46 | `chat_ui.rs:797` | `unsupported_decoder_format`, `media_backend_unclassified` |
| 61 | `export_ui.rs:1852` | the 4 `DeliveryColorError` codes, `unsupported_decoder_format`, `media_backend_unclassified` |

   `unknown_source_white_point` is reachable at row 27 on a measured path and not only in principle: `d65_assumption` fires only when `primaries == Bt709 && white_point == Unknown` (`crates/kinewright-media/src/render.rs:778-787`), so a source with `primaries: Srgb` and `white_point: Unknown` passes the primaries arm of `validate_source_fields` (`color.rs:724-757`), receives **no** assumption, and reaches `Err(UnknownWhitePoint)` **after** the assumption. IN1 §2.2 rule 6's recorded reason for excluding it — that the **bare** classifier reports it for every correctly tagged source — is about the bare classifier and is untouched; §0.2/d closes a reachable residue, not a settled ruling. IN1 §2.4 rule 32 still forbids raising an incident from the bare classifier.

15. **Every site whose message begins "Core actor stopped" takes `operation_internal`.** There are **14** at `d4ed8eb` — `app.rs:951`, `:1088`, `:1251`, `:1275`, `:1318`, `:1334`, `:1342`, `:1407`, `media_bin.rs:121`, `:139`, `media_workflow.rs:1104`, `:1318`, `:1974`, `inspector_ui.rs:862` — spread over the `Operations`, `Look` and `Relink` labels. Their placeholder body ("read the message, change what it names") is false: nothing the person changes will restart a stopped actor. `operation_internal`'s body is the true one and §5.6 widens it to name the stopped actor. This is the critic's S6, applied to every measured member rather than the six it named.

16. **`unknown_source_white_point` becomes the thirteenth `SourceColorIncident`** (§0.2/d). `SourceColorIncident::from_source_error` stops returning `None` for `ColorSourceError::UnknownWhitePoint`; IN1 §2.2 rule 6's "twelve incident codes from thirteen classifier variants" becomes thirteen from thirteen, and IN1 §2.2 rule 8's round-trip test asserts thirteen-to-thirteen with no `None` case. IN1 §2.3b rule 22's implementation erratum is reversed (§14 R9).

### 3.3 `IncidentSubject` gains six variants

17. The dedup key stays `(code, subject, observed)` (IN1 §2.3 rule 15), and `suppressed` is a `BTreeSet<(IncidentCode, IncidentSubject)>` (`incident.rs:840`), so the subject **is** the dedup axis, it **must** be `Ord`, and each variant **must** name the thing whose repeated failure is one problem:

```rust
pub enum IncidentSubject {
    Asset(AssetId),
    LutAsset(LutAssetId),   // the eighth variant — erratum A-R13
    Clip(ClipId),
    Track(TrackId),
    Chain(AudioChain),
    ExportJob,
    Project,
    Agent,
}
```

| variant | dedup axis — what makes two failures one problem | rows that build it |
| --- | --- | --- |
| `Asset(AssetId)` | one media file that keeps refusing | Media, Relink, Source monitor, Operations (per-asset) |
| `LutAsset(LutAssetId)` | one LUT in the project's look store that keeps refusing | Look (per-LUT) — erratum **A-R13** |
| `Clip(ClipId)` | one clip whose trim, speed or effect keeps failing | Operations (per-clip), Look (per-clip), the inspector's dynamic site |
| `Track(TrackId)` | one track's capture refusal | **row 124** only (`timeline_ui.rs:3266`, `RoomToneCaptureJob::track`), plus the **seven** track-addressed operations of rule 20 — erratum **A-R11** |
| `Chain(AudioChain)` | one bus or the master, whose mix or learn keeps refusing | the three Mixer rows 29–31, plus the chain-addressed operations of rule 18 |
| `ExportJob` (unit) | one export attempt, not one incident per variant it checks | Export |
| `Project` (unit) | a document-wide refusal with no narrower subject | Project, Look (store root), Recording, Media cache, Captions, crash recovery, Operations (actor-stopped) |
| `Agent` (unit) | the chat panel's own refusals | Agent, Agent branch, Branch preview |

   **Erratum (A-R13, 2026-09-16):** `IncidentSubject` declares **eight** variants, not seven, and this rule's "Look (per-LUT)" row against `Asset(AssetId)` is not constructible on the tree: a LUT is identified by a `LutAssetId`, which `model.rs`'s `id_type!` mints as a separate id space from `AssetId`, so a `Look` row cannot build `Asset(AssetId)` without inventing a correspondence that does not exist, and anchoring it to `Project` instead loses the card's anchor. `LutAsset(LutAssetId)` is therefore declared after `Asset` (`crates/kinewright-core/src/incident.rs:922`) with label `Look {n}` and wire shape `{"lut_asset":N}`; §3.3 rule 20's precedence, §3.3 rule 21's arm count, §3.11 rule 42's union, §7 items 11 and 13 and §9 clauses 5 and 16 all move with it, and §9 clause 16's ceiling loop quantifies over 67 × 8 = **536** pairs.

18. **`ExportJob`, `Project` and `Agent` are unit variants, and no new id type is minted** (N2.5/B2). The app runs **one** export at a time — `KinewrightApp::export_job` is a single `Option<ExportJob>` (`app.rs:245`), assigned at `export_ui.rs:1751`, and eleven of the thirteen `Export` rows fire from `export_delivery_document()` and the preflight **before** that line, so there is no job in existence to name. `AgentThread` (`chat_ui.rs:163-179`) has no id and threads are addressed by a `thread_index: usize` that is re-indexed when one is removed, so an index would be an unsound dedup axis. Their dedup axis is therefore `(code, observed)`, stated. Minting an `ExportJobId` in core would also collide with the agent crate's own `ExportJobId` (`crates/kinewright-agent/src/export_queue.rs:46`, imported at `server.rs:87`), which is a different id space; with unit variants the collision does not arise.

19. **`AudioChain` needs three derives it does not have.** `AudioChain` (`crates/kinewright-core/src/media.rs:1165-1169`, `Bus(AudioBusId) | Master`) derives `Debug, Clone, Copy, PartialEq, Eq, Hash` and **not** `PartialOrd`, `Ord` or `Serialize`, all three of which `IncidentSubject` requires (`incident.rs:267`). The implementer **must** add `PartialOrd, Ord, Serialize` to `AudioChain`'s derive list rather than introduce a wrapper: `AudioBusId` is an `id_type!` newtype that already derives all three (`model.rs:15-51`), so the derives are available, and `AudioChain` is serialised nowhere today, so adding `Serialize` moves no byte. `Deserialize`/`JsonSchema` are **not** added: `IncidentSubject` carries neither.

20. **The subject of an `OpError` or a `BatchError` incident is derived from the operation, not chosen by the caller** (N2.5/B7). Core gains:

```rust
#[must_use]
pub const fn Operation::incident_subject(&self) -> IncidentSubject;
```

   returning the **narrowest** id the operation addresses, by the precedence **`Clip` → `Asset` → `LutAsset` → `Track` → `Chain` → `Project`** (A-R13); `Project` where it addresses none. `AddClip { track, asset, .. }` therefore yields `Asset(asset)`, `SetTrackAutomation { track, .. }` yields `Track(track)` (§0.3 D3), `RemoveAudioBus { bus }` yields `Chain(AudioChain::Bus(bus))`, `SetAudioMaster { .. }` yields `Chain(AudioChain::Master)`, and `SetPanLaw { law }` yields `Project`. `BatchError::BatchRejected` yields the common subject when every operation in the batch agrees and `Project` otherwise. The app's drain arms at `app.rs:1578-1582` and `:1583-1589` stop discarding `op` / `operations` (`actor.rs:105-112`). One core test **must** be exhaustive over all **57** `Operation` variants with no wildcard; `operation_tools().len() == 54` is a different count and is unchanged (§6.4 rule 9). **Erratum (A-R13, 2026-09-16):** the precedence gains a rung, shown above, and exactly two of the 57 variants move off `Project` onto it — `AddLutAsset` (the id read off its carried payload) and `RemoveLutAsset` (`crates/kinewright-core/src/operation.rs:5225-5226`) — so `Project`'s membership is **14**, not 16. The rung discriminates nothing else on the tree: no `Operation` carries both an `AssetId` and a `LutAssetId`, and `ConvertLegacyLook`, which carries a `LutAssetId` beside a `ClipId`, answers `Clip`, which the test pins on values.

21. **`IncidentSubject::label()` (`incident.rs:283`) gains seven arms** returning `"Asset 1"`, `"Look 5"`, `"Clip 4"`, `"Track 2"`, `"Bus 3"` / `"Master"`, `"Export"`, `"Project"` and `"Agent"`. **Erratum (A-R13, 2026-09-16):** seven arms, not six, and the seventh is `LutAsset(lut) => format!("Look {lut}")` (`crates/kinewright-core/src/incident.rs:947`), matching the `Asset {n}` / `Clip {n}` style.

22. **`Subject(String)` is rejected**, and the reason is the dedup key: two spellings of one clip would become two problems.

### 3.4 `IncidentEvidence` and `probed() -> Option<&ColorDescription>`

23. `IncidentEvidence` gains one variant per evidence shape Part B needs (N1/B3). `probed()` **must** become `Option<&ColorDescription>`; it cannot stay `&ColorDescription` and be per-variant:

```rust
pub enum IncidentEvidence {
    /// Part A's variant, byte-identical on the wire (§3.11 rule 41).
    SourceColor { probed: ColorDescription, assumption: Option<ColorSourceProfileAssumption> },
    /// A refusal whose only typed facts are already in `observed`/`allowed`.
    Plain,
    /// A rejected edit, with the family so a consumer can branch without re-parsing.
    OpError { family: IncidentFamily, op_number: Option<usize>, message: String },
    /// A typed media failure that is not a source-colour one.
    MediaError { code: &'static str, message: String },
    /// A revision gate that refused (§4).
    Revision { expected: TimelineRevision, actual: TimelineRevision },
    /// The app-local and agent-local rejections, each carrying its own rendered reason.
    SourceEdit { reason: &'static str },
    Relink { reason: String },
    ProjectSave { reason: String },
    Branch { reason: String },
    CaptionPlan { reason: String },
    DeliveryVariant { reason: String },
}
```

24. **Evidence follows the code, by rule** (N2.5/B8). Where a producer's code is resolved at run time, the evidence variant is resolved with it, and a delegating variant **must not** fall back to a `reason` string:
    - `BatchError::Empty` supplies code `edit_plan_rejected` and evidence `OpError { family: IncidentFamily::Malformed, op_number: None, message }` — `Malformed` because an edit plan with no operations is a malformed plan, and the field is what makes `OpError` evidence buildable at all.
    - `BatchError::OperationFailed { op_number, error }` delegates its code to `error.incident_code()` (§3.1 rule 6) and supplies `OpError { family: error.incident_family(), op_number: Some(op_number), message }`.
    - `BranchError::InvalidBase(OpError)` delegates its code the same way and supplies **`OpError` evidence**, never `Branch { reason }`; `BranchError`'s other four variants supply code `agent_branch_rejected` and evidence `Branch { reason }`.
    - `DeliveryVariantError`'s `InvalidDocument(OpError)` arm behaves as `InvalidBase` does; its other two arms supply `delivery_variant_rejected` and `DeliveryVariant { reason }`.
    Appendix B's six run-time rows carry both pairings explicitly, and Appendix B's "Reading the table" paragraph states the rule.

25. **All ten `probed()` call sites, with their new form.** Measured at `d4ed8eb` and confirmed complete by probe-2b T9 (ten sites, no others; the three `probed_*` matches elsewhere are unrelated fixture helpers). Two are in core and eight outside it, two of the eight `#[cfg(test)]`.

| # | site | today | new form |
| ---: | --- | --- | --- |
| 1 | `core/incident.rs:712` (`policy_recovery`) | `let probed = evidence.probed();` | returns `Option`; the `AutoApply` arm is gated on `Some` (§3.7) |
| 2 | `core/incident.rs:899` (`observe`) | `policy_class(observation.code, observation.evidence.probed())` | `policy_class(observation.code, &observation.evidence)` (§3.5) |
| 3 | `app/app.rs:104` (`router_apply_accepted`, `Applied`) | `recovery_description(incident.evidence.probed())` | `let Some(probed) = incident.evidence.probed() else { return false };` before the match |
| 4 | `app/app.rs:107` (`router_apply_accepted`, `Reverted`) | `*incident.evidence.probed()` | same `let … else`, then `*probed` |
| 5 | `app/incident_ui.rs:94` (`card_action_outcome`) | `color_description == incident.evidence.probed()` | `incident.evidence.probed().is_some_and(\|probed\| color_description == probed)` (§5.5 rule 29) |
| 6 | `app/incident_ui.rs:203` (`card_actions`) | `incident.evidence.probed().clone()` | inside the `Some(probed)` arm the revert branch already needs (§3.8 break 3) |
| 7 | `app/incident_ui.rs:226` (`card_details`) | `let probed = incident.evidence.probed();` | `if let Some(probed) = incident.evidence.probed() { … }` around the `probed`/`assumed` rows only |
| 8 | `agent/server.rs:16476` (`verify_claimed_outcome`) | `let probed = incident.evidence.probed();` | inside the source-colour arm of the per-code verification (§6.1) |
| 9 | `app/app.rs:4436` (`#[cfg(test)]`) | `incident.evidence.probed()` | `.expect("the fixture incident carries a probed description")` |
| 10 | `app/app.rs:4754` (`#[cfg(test)]`) | `incident.evidence.probed()` | same |

### 3.5 `policy_class(code, &IncidentEvidence)`

26. The signature changes and the reason is measurable: an `operation_bounds` incident on a `Clip` subject has no probed colour description, so under the shipped signature (`incident.rs:680`) it cannot be classified at all.

```rust
#[must_use]
pub fn policy_class(code: IncidentCode, evidence: &IncidentEvidence) -> PolicyClass;
```

   `PolicyPredicate::Always` returns the declared class. `PolicyPredicate::Rec709Compatible` returns the declared class when `evidence.probed().is_some_and(rec709_compatible)` and `PolicyClass::Explain` otherwise — so a `Rec709Compatible` row reached with no probed description falls to `Explain`, which is the safe direction and the only honest one. `rec709_compatible` itself is **unchanged**, and IN1 §9 clause 19's equivalence test stays green unmodified.

27. **`IncidentLog::observe` (`incident.rs:899`) is the single site that *assigns* the class**, which is IN1 §2.3b rule 25's property and is unchanged. `policy_recovery` (`incident.rs:713`) *consults* it; it is the second caller and always was (the critic's S1).

### 3.6 `POLICY` gains a severity column

28. `PolicyEntry` is unchanged in shape: it **already carries** `severity: IncidentSeverity` (`incident.rs:583`) and `Incident` already serialises it (`incident.rs:401`), so N1/B4's column costs **zero wire bytes** — what Part B changes is the values, not the shape (confirmed by probe-2b T2). `POLICY` becomes `[PolicyEntry; 67]` in the declaration order of §3.2 rule 10's enum. Every Part B row declares `class: PolicyClass::Explain` and `predicate: PolicyPredicate::Always`; the three Part A `AutoApply`/`Rec709Compatible` rows are unchanged.

29. **IN1 §9 clause 1's "every row `Blocks`" is superseded**, with the reason written here as N0 wrote it for IN1 §10 rule 6: the clause was true of a twelve-row table in which every row stopped a managed decode, and it is false of a sixty-seven-row table in which thirteen rows report work that completed with a loss. The replacement clause is **"every colour row blocks, and every row's severity is the one §3.6 rule 30's table declares"**, asserted by equality over the whole table (§9 clause 2). IN1 §9 clause 1 takes erratum **R1a** at promotion.

30. **The severity rule, normative.** A row's severity answers one question: *did the thing the person asked for happen?*
    - **`Blocks`** — it did not. The site returns early without applying, leaves a command unsent, sets `playing = false`, or aborts an export, an import, a save or a restore.
    - **`Degrades`** — it happened, with a reduced result. The project opened, the export was written, the measurement ran — and something inside it is missing.
    - **`Informs`** — it happened and nothing is reduced.
    The value is derived from the site's own control flow at `d4ed8eb`, is declared **once per code** (N2/c), and Appendix B carries it per site so the derivation is checkable. Where two sites of one code would disagree, the code splits: that is why `look_incomplete` and `media_incomplete` exist (§0.2/c).

31. **The declared severities, by group.** 54 `Blocks`, 13 `Degrades`, 0 `Informs`.

| codes | severity | reason |
| --- | --- | --- |
| the 13 `SourceColor` rows | `Blocks` | a source-colour refusal stops the managed decode and no frame appears (IN1 §2.3b rule 26, unchanged) |
| `unsupported_decoder_format`, `media_backend_unclassified` | `Blocks` | the decode, import or playback the person asked for did not happen |
| the 4 `DeliveryColorError` rows | `Blocks` | the managed delivery encode was refused; nothing was written |
| the 5 `DeliveryVerificationError` rows | `Degrades` | the export **was** written; the verification could not produce an honest measurement (`delivery.rs:900-908`) |
| the 6 `ColorQcError` rows | `Degrades` | the measurement was refused and nothing was mutated — `color_qc.rs:704` says so in as many words for `NodeRemovalRejected` |
| the 11 `operation_*` family rows | `Blocks` | the edit was rejected; `Document::apply` changed nothing — and for `operation_internal`'s 14 actor-stopped sites the command was never sent |
| `lut_asset_policy`, `edit_revision_conflict` | `Blocks` | the operation was refused before it touched the document |
| `edit_plan_rejected`, `delivery_variant_rejected`, `agent_branch_rejected`, `source_edit_rejected`, `relink_rejected`, `project_save_failed`, `caption_plan_rejected` | `Blocks` | each site returns without applying; every `SourceEditRejection` message ends "no edit was applied" |
| the 15 label placeholders | `Blocks` | measured per site in Appendix B: every one returns early from the request |
| `look_incomplete`, `media_incomplete` | `Degrades` | the project saved or opened; some looks, some media or some timeline pictures did not come with it |
| — | `Informs` | **no Part B row.** Nothing measured is purely advisory; the variant stays declared for IN3 (§0.2/h, §13 D-B7) |

32. `POLICY`'s exhaustiveness test (`incident.rs:1169`) is rewritten against 67 rows and its test-local no-wildcard `ordinal()` (`incident.rs:1149`) grows with the enum. `in1_policy_rows_are_declared_in_table_order` (`incident.rs:1188`) indexes a `[PolicyEntry; 67]`.

### 3.7 `policy_recovery` becomes subject-generic

33. `pub fn policy_recovery(code, subject, evidence) -> Vec<RecoveryAction>` keeps its signature and stays the **single** producer (IN1 §2.5 rule 45), and its body stops destructuring the subject:

```rust
match policy_class(code, evidence) {
    PolicyClass::AutoApply | PolicyClass::AskFirst => {
        // The only applying recovery IN1 ships needs an asset and a probed
        // description; a row that resolves to AutoApply without both is a
        // contradiction the exhaustiveness test catches, not a panic here.
        match (subject, evidence.probed()) {
            (IncidentSubject::Asset(asset), Some(probed)) => vec![RecoveryAction {
                label: ASSUME_REC709_LABEL,
                kind: RecoveryKind::Operation(assume_rec709_operation(asset, probed)),
            }],
            _ => Vec::new(),
        }
    }
    PolicyClass::Explain => vec![RecoveryAction {
        label: EXPLAIN_LABEL,
        kind: RecoveryKind::Explain(explain_body(code)),
    }],
}
```

34. **`explain_body(code) -> &'static str` is core's and is exhaustive with no wildcard arm.** It **must** delegate to a shipped `recovery_action()` wherever one exists and **must not** restate its string. **Twenty-eight** codes delegate, and they carry **16** distinct sentences, not 28:

| accessor | codes | distinct sentences |
| --- | ---: | ---: |
| `DeliveryColorError::recovery_action()` (`delivery.rs:831`) | 4 | 4 |
| `DeliveryVerificationError::recovery_action()` (`delivery.rs:961`) | 5 | 5 |
| `ColorQcError::recovery_action()` (`color_qc.rs:778`) | 6 | 6 |
| **`ColorSourceError::recovery_action()` (`color.rs:328-332`)** | **13** | **1** |
| | **28** | **16** |

   `ColorSourceError::recovery_action()` is a `const fn` with **no `match`**: it returns *"Apply an explicit supported source-colour override or relink to compatible media."* for all thirteen variants. The thirteen codes are named here so nothing downstream claims otherwise: `unknown_source_primaries`, `unsupported_source_primaries`, `unknown_source_transfer`, `unsupported_source_transfer`, `unknown_source_matrix`, `unsupported_source_matrix`, `unknown_source_range`, `unsupported_source_range`, `unknown_source_white_point`, `unsupported_source_white_point`, `unknown_source_bit_depth`, `unsupported_source_bit_depth`, `unsupported_source_combination`. Rewriting them per code is IN1 §13 D8, owned by IN3 (§1 non-goal 13, §5.6 rule 32). `incident.rs:726-727`'s existing comment already records the shape. The remaining **39** rows have no accessor to delegate to and are written in §5.6, so the whole table carries **39 + 15 + 1 = 55** distinct sentences over 67 codes, and the **54 non-colour bodies are pairwise distinct** (probe-2b T1 measured all 54, longest 321 B). Part A's `Explain` arm reached `code.source_error().recovery_action()` (`incident.rs:730`); `explain_body` supersedes that call for every code and the thirteen colour rows keep the shipped sentence verbatim, so IN1 §9 clause 6's whole-table equality assertion stays true of them. **An empty `Vec` is a legal return** — it is what an `AutoApply` row with no asset and no probed description yields — and IN1 §9 clause 6 quantifies only over rows whose declared class is `AutoApply` or `AskFirst`, all three of which are asset-scoped colour rows.

### 3.8 The seven irrefutable-let replacements

35. Growing `IncidentSubject` and `IncidentCode` breaks exactly **seven** irrefutable `let`s across three crates (probe-1b P4). All seven **must** be replaced, and the `==` at `media_bin.rs:438` **must** be changed deliberately because the compiler will not ask. Three of revision 1's citations were wrong and are corrected here (the critic's S3).

| # | site at `d4ed8eb` | crate | breaks on | replacement |
| ---: | --- | --- | --- | --- |
| 1 | `core/incident.rs:715` (`policy_recovery`) | core | `IncidentSubject` | the `match (subject, evidence.probed())` of §3.7 |
| 2 | `app/app.rs:98` (`router_apply_accepted`, `fn` at `:93`) | app | `IncidentSubject` | `let IncidentSubject::Asset(asset_id) = incident.subject else { return false; };` — a non-asset subject has no colour recovery to land |
| 3 | `app/incident_ui.rs:195` (`card_actions`) | app | `IncidentSubject` | `let (IncidentSubject::Asset(asset), Some(probed)) = (incident.subject, incident.evidence.probed()) else { … }`, falling through to the `policy_recovery` branch |
| 4 | `app/incident_ui.rs:114` (`incident_headline`) | app | **`IncidentCode`** | the 70-row headline table of §5.5 rule 30. **This one fires before any subject work**, the moment `IncidentCode` grows its first non-colour row |
| 5 | `agent/server.rs:16456` (`verify_claimed_outcome`) | agent | `IncidentSubject` | the per-code verification of §6.1, which returns `None` for every code with nothing to verify |
| 6 | `app/app.rs:4423` (`#[cfg(test)]`) | app | `IncidentSubject` | `else { panic!("the fixture incident is asset-scoped") }` |
| 7 | `app/app.rs:4746` (`#[cfg(test)]`) | app | `IncidentSubject` | same |
| — | `app/media_bin.rs:438` | app | **nothing** | `incident.subject == IncidentSubject::Asset(asset_id)` still compiles and silently keeps the Media panel asset-only. §5.5 rule 31 makes the panel's asset filter explicit and routes every other subject to the Incidents panel |

### 3.9 `MediaError` gains two typed passthrough variants

36. `MediaError` (`media.rs:1739`) gains `MatteProof(MatteProofError)` and `MatteCoverage(MatteCoverageError)`, both `#[error(transparent)]`, and the two `From` impls at `media.rs:600` and `:659` stop flattening into `Self::Backend(error.to_string())`. `recovery_code()` (`media.rs:1826`) gains two arms delegating to `MatteProofError::code()` (`media.rs:589`) and `MatteCoverageError::code()` (`media.rs:648`), so it returns `Some` for **8 of 13** variants instead of 6 of 11 — **9 of 14** after erratum A-R12's `Store` variant. The rendered text is unchanged: both `code()` strings are already the first token of each `#[error]` template, so `matte_coverage_invalid_dimensions: …` renders identically with or without the `media backend error: ` prefix that `Backend` used to add — **and the prefix is the one visible change**, which is why §7 requires a media test asserting the new rendering. **Erratum (A-R12, 2026-09-16):** `MediaError` gains a **third** typed passthrough, `Store { code: &'static str, message: String }` with `#[error("media backend error: {message}")]` (`crates/kinewright-core/src/media.rs:1864-1873`, `recovery_code()` arm at `:1894`), ruled at N4/CR-D1 so that §6.2 rule 4's typed path and §9 clause 14 can be fully discharged: `LutStoreError`, `RoomToneStoreError` and `LutParseError` cross the media boundary through it carrying their own code, and `recovery_code()` is `Some` for **9 of 14** variants. The label stays in the template rather than in the three `From` impls, so `message` is the payload the eleven existing media-crate assertions read, the rendered text is byte-identical to the `Backend` string it replaces, and the 67-code set does not grow — `Store` routes to `media_backend_unclassified` exactly as the two matte variants do.

37. **Neither mints an `IncidentCode`** (§0.2/e). `IncidentObservation::from_media_error`'s exhaustive match (IN1 §2.3b rule 22) gains two arms taking `IncidentCode::Media(MediaIncident::BackendUnclassified)`, each with the one-line reason: the app reads both through `Result<_, String>` seams that never reach any of the three sinks §1 claims — `matte_overlay_ui.rs:100` and `color_scopes_ui.rs:1073`, both named in §13 D-B3. **Erratum (A-R1, 2026-09-16):** the two arms cannot return `None` and be reached through a **total** `from_media_error`, which §5.1 rule 12, §14 row A and IN1b-R9 all require; they take the already-declared `media_backend_unclassified` instead (`crates/kinewright-core/src/incident.rs:1306`), so no code is minted and the 67-code set is intact. Nothing is lost: `media_evidence` (`incident.rs:1433`) emits `code.code()` unconditionally, so one incident carries one code, and each matte error's own token survives as the first word of `observed`, because it is already the first token of both `#[error]` templates.

### 3.10 `CaptionPlanError` gets a code path

38. `crates/kinewright-app/src/captions.rs:74`'s `caption_title_operations` wrapper returns `Result<Vec<Operation>, String>` and throws away the `CaptionPlanError` (`crates/kinewright-core/src/captions.rs:68`) that core's own `caption_title_operations` (`crates/kinewright-core/src/captions.rs:88`) returns. The wrapper **must** return `Result<Vec<Operation>, CaptionPlanError>`, and `captions.rs:61` **must** build its observation from the typed value. This is the cheapest single typing win in the crate (N1.5 §2) and it is the only one of the seven `Result<_, String>` helpers whose inner error is already a core enum.

39. **Two of `CaptionPlanError`'s six variants take `operation_internal` instead** (the critic's S7). `TrackIdExhausted` and `ClipIdExhausted` (`crates/kinewright-core/src/captions.rs:68-81`) are id-space exhaustion; `caption_plan_rejected`'s body — *"adjust the transcript selection or the script"* — is false for them, and `operation_internal`'s is exactly right. `CaptionPlanError::incident_code()` therefore returns `IncidentCode::Operation(IncidentFamily::Internal)` for those two and `caption_plan_rejected` for the other four. This is the hybrid rule (N0/Q3) used the way §3.1 rule 6 uses it for the two LUT-asset variants, and it mints no code.

40. The other six helpers probe-1b named — `selected_transcript_word_cut_operations` and `transcript_word_cut_operations` (`transcript_edit.rs:126`, `:138`), `clip_speed_operations` (`timeline_ui.rs:2648`), `lut_import_operations` (`media_workflow.rs:1637`), `start_recording` (`recording.rs:169`) — and the two the probe missed, `linked_trim_operations` (`timeline_ui.rs:2468`) and `freeze_frame_operations` (`timeline_ui.rs:2545`), compose their own sentences from literals. Their sites take the label placeholder and Part B **must not** invent a typed error for them; **D-B5** (§13) owns it.

### 3.11 The wire shape, and the two size constants

41. **Part A's `SourceColor` incident must serialise byte-identically**, so IN1 §6.2 rule 10's literal still pins after Part B. Probe-2b T2 proved it rather than argued it: the shipped `in1_untagged_vp9.webm` fixture built through the real `IncidentLog::observe` and the same incident built through a field-for-field Part B prototype produce JSON bodies **equal as strings**, at **819 B**. Three things are load-bearing and the implementer **must not** change any of them: `IncidentEvidence` keeps `#[serde(rename_all = "snake_case")]` and its `SourceColor` variant keeps its name and both field names; `IncidentCode` keeps the hand-written `Serialize` that emits only `code()`; and `PolicyEntry`'s severity value for the colour rows is still `Blocks`, so the `"severity":"blocks"` key does not move. IN1 §9 clause 15's test stays green **unmodified**, and §9 clause 15 of this contract re-asserts it.

42. **The `IncidentSubject` wire union is stated, not discovered.** `Asset`, `Clip` and `Track` serialise as one-key objects over a transparent `u64` — `{"asset":1}`, `{"clip":4}`, `{"track":2}`; `Chain` serialises as a one-key object over a **nested** union, `{"chain":{"bus":3}}` or `{"chain":"master"}`; and `ExportJob`, `Project` and `Agent` serialise as the bare strings `"export_job"`, `"project"` and `"agent"`, because serde's `rename_all = "snake_case"` renders a unit variant as a string. An agent that learned `incident.subject.asset` therefore meets a field that is sometimes an object, sometimes a nested object and sometimes a string. **This is accepted, not fixed**: a payload-carrying `Project(ProjectId)` would need a project id the incident log does not have (the log lives on `ProjectSession` and is per-project by construction — IN1 §5.1 rule 1), and inventing one would put a second identity space on the wire for no reader. `get_incidents`' description (IN1 §6.4 rule 20) gains one clause naming all three shapes, and §7 pins **three** wire bodies byte for byte — an asset-subject, a project-subject and a chain-subject incident. **Erratum (D-R64, 2026-09-16):** `get_incidents`' description is **not** edited and the clause lives in core's `Incident` doc comment instead (`crates/kinewright-core/src/incident.rs:900-903`): the description is schema-visible, so one added clause would move `get_incidents`' pinned `1 145 / 493 / 492 / 236` tuple and the registry sextuple's `1 551 301` and `121 315`, which §6.4 rule 7 asserts unmoved and §12 lists under **Never cut**. The pins win (ruled at N4). **Erratum (A-R13, 2026-09-16):** the union's first shape gains a second member — `LutAsset` renders `{"lut_asset":N}` — and §7 item 13's test pins a **third** literal beside the project and chain bodies while keeping its declared name.

43. **`IN1_INCIDENT_SERIALIZED_BYTES` = 819** and **`IN1_INCIDENT_SERIALIZED_CEILING_BYTES` = 2 048** (`server.rs:16393`, `:16402`), both measured by probe-2b T2 and no longer markers. The worst case over the declared code set × the declared subject shapes is **1 354 B**, at `unsupported_decoder_format` on the largest subject, against the shipped 1 024 B ceiling, which **fails**; 2 048 is the smallest power of two above it and leaves **694 B (34 %)** of headroom, against Part A's 203 B (20 %) and probe-1b's 38 B (3.7 %). `server.rs:26186`'s `worst > IN1_INCIDENT_SERIALIZED_CEILING_BYTES / 2` assertion **holds unmodified** at 2 048 (1 354 > 1 024), so revision 1's conditional erratum is **struck**: no erratum is needed and none is written. The re-measurement after §3.3's subject shapes changed was the contract's one remaining unmeasured number, and it is now **measured**: over the 67 declared codes × the **eight** subject shapes of §3.3 rule 17 as amended by A-R13 — **536** pairs — core measures a worst of **1 351 B** at `unsupported_decoder_format` on `Chain(AudioChain::Bus(AudioBusId(u64::MAX)))` (`crates/kinewright-core/src/incident.rs:4033`), and the agent's independently written ceiling loop measures **1 358 B** on the same pair (`crates/kinewright-agent/src/server.rs:26204`), the 7 B difference being review-2 N-3's real 77 B `ColorQcError::NodeRemovalRejected` `allowed` literal in place of core's 70 B synthetic. Both are under **2 048** and both are above 2 048 / 2, so `worst > CEILING / 2` holds unmodified; **`IN1_INCIDENT_SERIALIZED_BYTES` stays 819**, and `LutAsset(LutAssetId(u64::MAX))` is 3 B narrower than the worst pair, so the eighth subject did not move it. The headroom on the `Open`, telemetry-skipped shape is **690 B (34 %)**; review-2's saturated resolved-with-full-telemetry shape measures 1 737 B, still 311 B (15 %) under the same ceiling. **Erratum (A-R6, 2026-09-16):** implementer A measured both constants but did not move them, because moving the ceiling alone reddens the gate — at 2 048 the surviving twelve-code loop's worst of about 819 B fails `server.rs`'s `worst > CEILING / 2`, so the change is only green as one commit with implementer D's rewrite of §9 clause 16's loop. The measurement is carried in core instead, by `in1b_every_code_fits_the_measured_ceiling_on_every_subject_shape` (`incident.rs:4033`), and handed to D; **819 and 2 048 both stand**.

44. **What drives the size, and the limit.** The 368 B rise over probe-1b's 986 B is almost entirely the `recoveries` array: probe-1b measured against Part A's one shared 81 B colour sentence, and §5.6's written bodies run to **321 B**. `observed` is not bounded by the type system — `UnsupportedDecoderFormat` interpolates a file path and `app.rs:775`'s `media_incomplete` observation joins *every* missing file name (§5.1 rule 14) — so 1 354 B is a measurement over chosen realistic inputs, not a proof, exactly as Part A's and probe-1b's figures were.

---

## 4. The correlation token

1. **The gap, measured.** `Event::RevisionConflict { expected, actual }` (`crates/kinewright-core/src/actor.rs:113-116`, produced at `actor.rs:425`) carries no command identity, so `reconcile_router_sends` (`crates/kinewright-app/src/app.rs:1103`) can only match a conflict to an outstanding send by `(project_index, expected)` — preferring a candidate the live document does not show as landed, and falling back to the first candidate when every one does (`app.rs:1131-1149`). The fallback **is** IN1 §9 clause 9's case: a person made the same edit by hand first. The residual ambiguity — a foreign `DoIfRevision` refused at the same `expected` in the same frame as a landed router apply — is pre-existing and rare **today**, when the router sends one operation for one code; Part B multiplies router sends across 67 codes and seven subject kinds, and a two-tier heuristic over a larger candidate set is a heuristic that will be wrong.

2. **`Command::DoIfRevision` gains an optional correlation token**, and `Event::RevisionConflict` and `Event::DocumentChanged` echo it:

```rust
Command::DoIfRevision { expected: TimelineRevision, operation: Operation, token: Option<CommandToken> },
Event::RevisionConflict { expected: TimelineRevision, actual: TimelineRevision, token: Option<CommandToken> },
Event::DocumentChanged { doc, revision, last_op, journal_command, token: Option<CommandToken> },
Event::OpRejected { op: Operation, error: OpError, token: Option<CommandToken> },  // erratum A-R15
```

   `pub struct CommandToken(pub u64)` is a core newtype beside `TimelineRevision` (`actor.rs:37`), `Debug + Clone + Copy + Eq + Hash`, and carries **no `Serialize`**: it is an in-process correlation id, not a wire value. Probe-2b T3 confirms the shape is free structurally as well as by measurement — `Command` (`actor.rs:39`) and `Event` (`actor.rs:96`) carry no serde derive, no `JsonSchema`, no size assertion and no snapshot, and the serde-pinned command shape is `JournalCommand` (`journal.rs:14`), which has no `DoIfRevision` variant at all. **Erratum (A-R15, 2026-09-16):** the field list is short by one and `Event::OpRejected` gains the token too, as shown above (`crates/kinewright-core/src/actor.rs:117-131`, echoed at `:483`). A revision-gated send ends **three** ways — the gate refuses, the operation lands, or the gate passes and `Document::apply` rejects the operation — and this rule gave identity to only two of them, so rule 7's identity matcher had no answer for a refused-on-apply send and the app measured the cost: the send stayed outstanding for the session. `Event::BatchRejected` does **not** gain one, on rule 4's unchanged reason — its senders all use the synchronous `Core::request`, whose private reply channel is already an identity.

3. **`Option<CommandToken>` rather than a required field, and the reason is semantic, not editorial.** `Command::DoIfRevision` has four senders outside the actor at `d4ed8eb` — `app.rs:1082` (the router's auto-apply in `audit_new_incident`), `app.rs:1245` (`send_incident_recovery`, the card's control), `media_workflow.rs:1096` (the verified Source patch) and `media_workflow.rs:1310` (relink) — plus the agent's synchronous `core.request(...)` at `crates/kinewright-agent/src/server.rs:1582`, which reads its own reply and needs no token. Revision 1 argued that `Option` saves those edits; probe-2b T3 measured that it does not — a struct variant's field cannot be omitted, so a required `CommandToken` would edit exactly the same five construction sites, and `server.rs:1582` needs its one-line change either way. What `Option` buys is the **meaning** *"this send does not care"*, rather than forcing four senders to invent an id nobody reads. The conclusion stands on that reason.

4. **`Command::DoBatchIfRevision` does not gain a token, and the reason is its senders' transport, not their absence.** It has **four** production senders at `d4ed8eb` — `crates/kinewright-app/src/chat_ui.rs:703` (`cherry_pick_agent_branch`, **in the app crate**), `crates/kinewright-agent/src/branch.rs:184`, `crates/kinewright-agent/src/server.rs:1666` and `:2486` — plus three test senders and the dispatcher arm at `actor.rs:398`. Revision 1's claim that it "has no sender in the app crate" was false and its grep was wrong; the conclusion survives on the true reason. All four use the synchronous `Core::request` (`actor.rs:177`), whose private per-call `unbounded()` reply channel (`actor.rs:178-182`, replied at `:379`) is already an identity. The token exists for the fire-and-forget `Core::send` path, which only `DoIfRevision` uses. If Part B's migration ever sends a batch fire-and-forget, it takes the token in the same shape and the rule moves with it.

5. **The actor echoes and never invents.** `execute_command` (`actor.rs:385`) threads the token from the command into whichever event it returns — `RevisionConflict` from `revision_conflict` (`actor.rs:423`), `DocumentChanged` from `execute_operation` (`actor.rs:430`). Every other command path emits `token: None`, including the initial snapshot `DocumentChanged` at `actor.rs:361`. A core test **must** assert both directions: a `DoIfRevision` with a token that conflicts echoes that token on `RevisionConflict`, and one that lands echoes it on `DocumentChanged`. **Erratum (A-R15, 2026-09-16):** "whichever event it returns" is now true of all three, not two: `execute_operation`'s `Err` arm stops dropping the token (`crates/kinewright-core/src/actor.rs:483-487`), and `in1b_a_correlation_token_is_echoed_on_a_rejection` (`actor.rs:817`) asserts the third direction while `Command::Do` still answers `None` by construction.

6. **The app drain arm carries the token into the conflict note, and `RouterConflict` loses a field.** `RouterConflict` (`app.rs:63`) gains `token: Option<CommandToken>` **and drops `expected`**: once the match is by identity nothing reads `expected`, and `dead_code` under the house `-D warnings` gate is a build error, so the removal is forced in the same commit (probe-2b T3). `RouterApply` (`app.rs:77`) gains `token: CommandToken`, not an `Option`: every router send carries one, allocated from a `next_command_token: u64` counter beside `next_project_id` (`app.rs:118`).

7. **`reconcile_router_sends` matches by identity and the two-tier match is deleted.** The body becomes: find the outstanding `RouterApply` whose `token` equals the conflict's `Some(token)`; if there is none, **do nothing** — the conflict belonged to a foreign send and the `edit_revision_conflict` incident at Appendix B row 26 is the whole response. The `candidate`/`landed` closures, the `.or_else(…)` fallback at `app.rs:1147` and the long comment justifying it are removed, and `router_apply_accepted` (`app.rs:93`) keeps its one remaining caller, `reconcile_project_sends` (`app.rs:1180`). **Eight match arms destructure `Event::RevisionConflict` and none uses `..`**, so all eight break and all eight are in the budget: `branch.rs:193`, `server.rs:1602`, `:1697`, `:2509`, `app.rs:1591`, `app.rs:4286` (`in1_drain_core`, the test drain mirror §9 clause 10's tests run through), **`recovery.rs:1011`** and `actor.rs:697`. Of the 127 `DocumentChanged` sites only nine lack a `..` rest pattern and none of those is in `kinewright-media`, so implementer B still edits no file. **Erratum (C-R53, 2026-09-16):** the drain's `Event::OpRejected` arm does the same thing for the third way a gated send can end — it drops the outstanding `RouterApply` the echoed token names and raises **no** Appendix B row 24 card for it, because telling the person "this edit was refused; make it again" about an edit Kinewright planned and sent is the same lie C-R51 removes for conflicts. The incident stays `Open`, nothing is re-sent, its `revision` does not move, and a foreign rejection — carrying no token, or a token the router does not hold — behaves exactly as it did. The decision has **one owner**, `KinewrightApp::note_core_event_for_router` (`crates/kinewright-app/src/app.rs:1314`, over `router_send_for` at `:1268` and `drop_router_send` at `:1288`), called by the production drain at `:1759` and by the test mirror at `:4689`, so neither copy can drift from the other. **Erratum (C-R54, 2026-09-16), narrowing C-R51 and C-R53:** both skips argue from *"an edit Kinewright planned and sent"*, which is true of an auto-apply and false of a button the person pressed — and `send_incident_recovery` sets no status on either path, so a card press that lost its race did **nothing at all**: no card, no status line, no audit entry, where at `b99c328` the person got a card. `RouterApply` therefore carries `origin: RouterSendOrigin { Auto, CardPress }` (`app.rs:93`, the field at `:118`, set at `audit_new_incident` `:1249` and `send_incident_recovery` `:1567`) and **both skips apply only to `Auto`**; for a `CardPress` the apply is still dropped, but the observation is raised against **the incident's own subject**, because the person pressed a button on that card and that is where the answer belongs. A foreign event is unchanged — `Project` for a conflict, `op.incident_subject()` for a rejection — and `in1b_a_refused_card_press_is_reported_where_an_auto_apply_is_not` (`app.rs:6557`) asserts both halves.

8. **IN1 §9 clause 9 still holds, and is strengthened.** Its case — a router auto-apply issued against a stale revision leaves the incident `Open` with its `revision` refreshed and no second apply — is unchanged; what changes is that the refresh now reaches the incident the conflict actually refused rather than the first plausible one. IN1's own test for it, `in1_a_conflict_is_matched_to_the_refused_send_not_the_landed_one` (`app.rs:4603`), **must** be rewritten to assert the identity match. Revision 1 claimed that rewritten test must fail against the two-tier matcher; it does not, and the claim is withdrawn. The scenario it exercises — two `AutoApply` incidents on two assets at one revision, the first landed, the second refused — is exactly what the `.position(|apply| candidate(apply) && !landed(apply))` tier (`app.rs:1146`) already handles, which is why it passes today. **The discriminating test is the second one**, `in1b_a_foreign_conflict_leaves_every_router_send_outstanding`, which fails at `d4ed8eb` because the `.or_else(…)` fallback consumes a send that no conflict of the router's refused. §9 clause 10 fails as a whole on that test, and the clause says so rather than claiming both halves discriminate.

9. `Event::DocumentChanged`'s token is what lets `reconcile_project_sends` resolve a landed apply without reading the document, and Part B **may** use it that way; it **must not** remove `router_apply_accepted`'s document read, because `resolve_incident` verifies the same claim the same way (IN1 §6.3 rule 16) and the two must not disagree. **Erratum (C-R50, 2026-09-16):** the token becomes the **first of two** conditions rather than an optional shortcut — a router apply is recognised as landed only when an `Event::DocumentChanged` carrying its token has arrived **and** the document read agrees (`pending_router_landings` at `crates/kinewright-app/src/app.rs:264`, recorded at `:1618`, consulted at `:1338-1348`). Without it, a person who hand-wrote the recovery's bytes in the tick between send and conflict resolved the incident `Applied` off their own edit; Part A's drop-on-`moved_on` goes with it, because a send the actor has not answered is *in flight* rather than superseded, and `RouterApply::expected` is deleted with it, since neither decision reads it any more.

10. **The cost is measured, not estimated.** Probe-2b T3 implemented rules 2–8 in full in a detached worktree: **172 insertions / 73 deletions over 38 sites, 7 files, 3 crates, 51 hunks**, with `cargo check --workspace --all-targets` and `cargo clippy --workspace --all-targets -- -D warnings` both clean, **no pinned byte, schema or wire literal moved**, and all 15 app `in1_` tests green unmodified.

---

## 5. The app

### 5.1 The migration, family by family

11. Part B's order **must** be IN1 §10 rule 3's — typed families first, because each brings a `code()` with it and each step reuses the previous step's observation-constructor shape. **The `sites` column counts the sites the step touches, including untyped siblings inside the same function**, and is not Appendix B's evidence column, which counts typed payloads; the two disagree by construction and this sentence is which one governs an implementer's commit boundary (the critic's S12). Appendix B's evidence column has five `MediaError` rows; step 1 touches nine sites.

| step | what | sites | brings |
| ---: | --- | ---: | --- |
| 1 | `MediaError`'s sites and their siblings | 9 | `recovery_code()` is `Some` for 8 of 13 after §3.9 — **9 of 14** after A-R12's `Store` variant; the code-less variants take `media_backend_unclassified`; `from_media_error` becomes total (rule 12) |
| 2 | Export's typed sites | 4 | `DeliveryVariantError` ×3 (delegating to `OpError` on `InvalidDocument`), `MediaError` ×1 |
| 3 | Relink's typed sites | 5 | `RelinkRevisionConflict` ×3 → `edit_revision_conflict`; `RelinkRejection` ×2 |
| 4 | Agent branch's typed sites, and §5.7's two chat sinks | 7 | `BatchError` ×2, `BranchError` ×3, both delegating to `OpError`; the two `BranchApplyOutcome::Conflict` arms |
| 5 | Source monitor and Project's typed sites, and §5.7's restore sink | 3 | `SourceEditRejection`, `ProjectSaveError`, `restore_status` |
| 6 | Captions' typed site | 1 | `CaptionPlanError` (§3.10) |
| 7 | **`OpError`** | 3 | the 11 families through `app.rs:1580`, `:1585` and `Operation::incident_subject()` (§3.3 rule 20) |
| 8 | the string-only remainder, and the sink deletion | 100 | one `Explain` placeholder per label, `operation_internal` for the 14 actor-stopped sites (§3.2 rule 15), then rule 20's deletion |
| | **total** | **132** | |

12. **`IncidentObservation::from_media_error` becomes total, and `app.rs:1637`'s `else` arm disappears rather than being migrated** (the critic's S9). Today the site is the `else` of `if let Some(observation) = IncidentObservation::from_media_error(&error, revision) { … } else { record_error("Media", …) }` (`app.rs:1631-1641`), so `SourceColor` evidence and an `AutoApply` class — which Appendix B row 27 declares — are precisely the cases that cannot reach it. Once every `MediaError` variant has a code (§3.2 rule 12, §3.9), the constructor returns `IncidentObservation` rather than `Option<IncidentObservation>`, both arms fold into one, and row 27 describes the merged site. §2.1 rule 2's "a site made conditional is not migrated" is not in tension: the site is not made conditional, it ceases to exist. **Erratum (A-R2, 2026-09-16):** a total constructor needs a fallback subject, so the signature is `from_media_error(&MediaError, IncidentSubject, TimelineRevision) -> IncidentObservation` (`crates/kinewright-core/src/incident.rs:1306`), where only `MediaError::SourceColorForAsset` knows its own subject and overrides the caller's fallback; §14 row A should name the parameter. **Appendix B row 27's subject cell is amended by this erratum**: `Asset` for the thirteen `SourceColor` codes and **`Project`** for `unsupported_decoder_format` and `media_backend_unclassified`, because `UnsupportedDecoderFormat` carries a `PathBuf` and `Backend` a `String` and neither has an `AssetId` to build `Asset` from — which is what the migrated sink passes (`crates/kinewright-app/src/app.rs:1658`) and what implementer C's per-label `Media` test asserts.

13. **File-by-file migration is rejected on measured grounds** (the brief critic's S9): the 124 literal sites live in **14 files** and the label-to-file map is not one-to-one — `Operations`' 34 sites are spread across `app.rs`, `keys.rs`, `timeline_ui.rs`, `media_bin.rs`, `inspector_ui.rs` and `media_workflow.rs`. File-by-file therefore **cannot** land one code decision per commit, which is the property family-first buys.

14. **The three `ErrorLog::push` bypasses are aggregates and are not mechanical.** `app.rs:760` joins a `Vec<String>` of LUT titles, `app.rs:775` joins a `Vec<String>` of missing media **inside** the `self.status = if missing.is_empty() { … } else { … }` expression, and `app.rs:1377` loops once per `(asset, error)` from `self.visual_cache.poll(ctx)`. Their shapes are fixed here: the two joins become **one** incident whose `observed` is the joined list, because one open with *n* missing files is one problem; the `visual_cache` loop becomes **one incident per asset**, subject `Asset(asset)`, because a per-asset failure is per-asset by construction and the dedup key collapses repeats correctly. The `app.rs:775` push **must** be lifted out of the `if` expression before the status line is composed. **This changes visible behaviour and the change is wanted** (the critic's S15): at `d4ed8eb` these three push directly and never set `error_log_open`, so the lines land in a closed window and the person sees only the status string; after §5.3 every incident routes through `note_incident`, which opens the window. Opening a project with missing media will therefore pop the Incidents surface where it did not before — which is the whole point of a badge that counts problems, and is what §11 item 5 hand-runs.

### 5.2 `IncidentObservation` grows its constructors

15. `IncidentObservation` (`incident.rs:439`) keeps its fields and gains constructors beside `from_media_error` (`incident.rs:462`), each `#[must_use]` and each in core:

```rust
impl IncidentObservation {
    pub fn plain(code: IncidentCode, subject: IncidentSubject, observed: impl Into<String>,
                 revision: TimelineRevision) -> Self;
    pub fn from_op_error(error: &OpError, subject: IncidentSubject,
                         revision: TimelineRevision) -> Self;
    pub fn from_batch_error(error: &BatchError, subject: IncidentSubject,
                            revision: TimelineRevision) -> Self;
    pub fn revision_conflict(subject: IncidentSubject, expected: TimelineRevision,
                             actual: TimelineRevision) -> Self;
}
```

   The `subject` parameter stays, because a caller that knows better than the operation keeps the right to say so; what changes is that **no caller invents one**. `app.rs:1580` passes `op.incident_subject()` and `app.rs:1585` passes the batch's common subject (§3.3 rule 20). The four out-of-core enums of §2.2 rule 8 each gain an inherent `fn incident_observation(&self, subject, revision) -> IncidentObservation` in their own crate. IN1 §2.3b rule 21's "nothing outside `incident.rs` constructs an `Incident`" is unchanged: these build *observations*, and `IncidentLog::observe` is still the only way an incident exists.

16. `pending_observations` (`app.rs:188`) becomes `pub(crate)`. It is the sink's replacement seam and every migrated site outside `app.rs` writes to it; probe-1b P5 measured that this is one line and that no other plumbing is needed. **Erratum (C-R47, 2026-09-16):** four `note_*` shorthands sit beside `note_incident` — `note_observation`, `note_plain`, `note_label` and `note_actor_stopped` (`crates/kinewright-app/src/error_ui.rs:104`, `:114`, `:127`, `:142`) — because making 128 sites write the four-argument constructor inline would have cost roughly 380 lines and repeated the focused-project revision read at every one of them. `pending_observations` is `pub(crate)` regardless, as this rule requires, and the tests write to it directly.

### 5.3 The sink gate

17. **`KinewrightApp::record_error` (`error_ui.rs:64-69`) is deleted, not wrapped, at the end.** A deprecated wrapper is a sink the next slice will use. It does three things today and all three **must** survive, in one replacement:

```rust
// `error_ui.rs`, the single writer.
pub(crate) fn note_incident(&mut self, incident: &Incident) {
    let headline = incident_headline(incident.code, incident.class);
    self.status = headline.to_owned();          // the status bar
    self.error_log.push("Incident", headline);  // the audit line
    self.error_log_open = true;                 // pops the window open
}
```

   `note_incident` is a method on `KinewrightApp`, **not** on `ErrorLog`, because `status` (`app.rs:243`) and `error_log_open` (`app.rs:249`) are `KinewrightApp` fields and a method on the log cannot reach them (N1/B5). `ErrorLog::push` (`error_ui.rs:31`) drops from `pub(crate)` to module-private `fn push`, with `note_incident` as its only caller.

18. **`note_incident` has exactly one call site**, in `audit_new_incident` (`app.rs:1050`), replacing the `self.record_error("Incident", headline);` at `app.rs:1074`. The borrow is already safe and Part A built the reason: `audit_new_incident` takes its guard from a **local** `Arc` (`app.rs:1056`), so `&mut self.error_log` is disjoint from it, and the existing `drop(log)` at `app.rs:1073` is hygiene rather than necessity. Probe-2b T4 built this and it compiles.

19. **A test asserts the two behaviours the deletion would otherwise drop silently.** After routing one fixture incident, `app.status` equals that incident's headline and `app.error_log_open` is `true`. No test in the app crate reads `app.status` at `d4ed8eb` — 34 assertions mention `status` and none reads the field — so without this clause both regressions ship under a green gate (N1/B5).

20. **The gate, spelled as four exact greps with definitions excluded, asserted by a test.** A gate a test asserts must carry the pattern, not a paraphrase (N2.5/probe 3, 4, 11). The `d4ed8eb` column below was measured by this revision with these exact commands.

| # | the grep, exactly | at `d4ed8eb` | after Part B |
| ---: | --- | ---: | ---: |
| 1 | `grep -rn 'record_error(' crates/kinewright-app/src \| grep -v 'fn record_error(' \| wc -l` | **126** | **0** — the symbol is gone |
| 2 | `grep -rn 'error_log\.push(' crates/kinewright-app/src --include='*.rs' \| grep -v '^crates/kinewright-app/src/error_ui\.rs:' \| wc -l` | **3** (`app.rs:760`, `:775`, `:1377`) | **0** |
| 3 | `grep -c 'error_log\.push(' crates/kinewright-app/src/error_ui.rs` | **1** | **exactly 1** — the `note_incident` body |
| 4 | `grep -rn 'note_incident(' crates/kinewright-app/src \| grep -v 'fn note_incident(' \| wc -l` | **0** | **exactly 1** — in `audit_new_incident` |

   Three spellings are ruled out and the reason for each is written so nobody re-derives them. A bare `grep -c '\.push('` in `error_ui.rs` reads **2** before **and** after, because `ErrorLog::push`'s own body is `self.entries.push(…)` (`error_ui.rs:32`); count 3 is therefore the literal `error_log.push(`. A grep that counts definitions makes count 1 read 127 and count 4 read 2, which is why all four exclude the `fn` line — count 1's 126 is call sites and nothing else. And `ErrorLog::push` path-qualified, the design brief's spelling, reads **0** workspace-wide before and after, so it would be a clause that passes before the slice exists (N1.5 §12).

21. **`record_error` survives as an unchanged shim through steps 1–7 and is deleted in step 8** (N2.5/probe 5). "Deleted, not wrapped" is the **end state**, and the gate asserts it at the **final** app commit only. The reason is measured: probe-2b T4 deleted the symbol and privatised `push` over a tree with seven sites already migrated and got **121** compile errors in `cargo check -p kinewright-app` — 118 `E0599` (no method `record_error`) plus 3 `E0624` (`push` is private, at the three bypasses), **239** under `--all-targets`. §9 P1 runs `cargo check -p kinewright-app` before **every** commit, so a single commit carrying all 132 sites plus the sink would be §12's whole ≈1 650-line centre in one unreviewable change. `record_error` therefore stays, **unchanged and undeprecated** — not `#[deprecated]`, which would fail `-D warnings` at its own call sites — through steps 1–7; step 8 migrates the last sites, deletes the symbol and privatises `push` in one commit, and that commit is where §9 clause 6 is discharged.

22. **What the gate does not claim, written beside it.** All four counts are greps over the app crate. `crates/kinewright-agent/src/server.rs` has **277** `error_text(`/`error_structured(` call sites at `d4ed8eb` (probe-2b T7: 252 + 25, definitions excluded), every one of which reaches a person through the chat panel and only **22** of which carry a `code` key. The gate reads 0/0/1/1 while they do. They are a different population, they are deferred by name (§13 D-B1), and the gate's doc comment says so in one sentence so nobody reads 0/0/1/1 as "no untyped refusal reaches a person". The same sentence names §13 D-B2 and D-B3, the app-crate sinks the gate does not see either.

23. **A code with no typed payload is still an incident.** IN1 §2.3c rule 28 — "an error with no `IncidentCode` is not an incident; it stays a `record_error` line" — is **superseded**: after Part B there is no `record_error` line to stay as. Every measured site has a code in Appendix B, and IN1 §2.3c rule 28 and its twin §2.2 rule 9 take errata **R2** and **R12**.

24. `#[cfg(test)] ErrorLog::count_with_source` (`error_ui.rs:51`) stays; it is the two-counts test's reader and probe-1b measured that all seven surviving `error_log` assertions use it.

### 5.4 The badge, and the two dynamic sites

25. **The badge reads `open_count()`.** `app.rs:1875` and `:1879` count `self.error_log.len()`; both become the focused session's `IncidentLog::open_count()` (`incident.rs:1029`). The error-log window keeps rendering `self.error_log.entries.len()` (`error_ui.rs:82`) and keeps its name, because it is now genuinely an audit view. IN1 §5.2 rule 15's sentence — *`open_count()` counts problems currently unresolved; `ErrorLog::len()` counts lines ever written this session* — becomes the badge's doc comment, and IN1 §1 item 10 and §13 D4 are discharged. A badge summing both is rejected: it double-counts every auto-applied incident, the one case IN1 exists to make invisible. **Erratum (C-R49, 2026-09-16):** the badge's **visibility** is decided separately from its number, which this rule does not state. Rendering on `open_count() > 0` alone made the audit window unreachable the moment the last incident resolved, so `incident_badge(open_count, audit_entries)` renders whenever either quantity is non-zero (`crates/kinewright-app/src/incident_ui.rs:493`, read at `app.rs:2147`) and the Incidents panel it opens carries an **Audit log** button, disabled when nothing has been recorded (`incident_ui.rs:607`). The number is unchanged: always the open count.

26. `in1_the_open_count_and_the_audit_line_are_different_quantities` (`app.rs:4694`) moves with the badge and **must** assert the badge's number — `open_count()` — and not only the log's, so it fails against a badge that still reads `len()` (N1/B6).

27. **Both dynamic sites are typed at their producers.**
    - `app.rs:470` is **not** dynamic in substance: both `load_error` producers (`app.rs:279`, `:287`) write the literal `"Project"` (N1.5 §5). `load_error` becomes `Option<IncidentObservation>`, built at the two producers, and the single consumer pushes it onto `pending_observations`.
    - `inspector_ui.rs:793` runs `for message in edits.errors { self.record_error(category, message) }`, so *n* errors under one category become *n* unrelated log lines today. `InspectorEdits::error_category()` (`inspector_ui.rs:238`) becomes `fn incident_code(&self) -> IncidentCode`, returning `look_unclassified` by default and `timeline_unclassified` where `set_error_category(ROOM_TONE_ERROR_CATEGORY)` is called (`timeline_ui.rs:3133`, `:3275`). The loop becomes one observation per message, and the incident key `(code, subject, observed)` collapses the duplicates correctly — **which is a live bug fixed in kind, and is the argument for typing at the producer**; the sink gate reasons about no site's label and could not tell the difference. **Erratum (C-R44, 2026-09-16):** the two `&'static str` category constants the accessor returned have no remaining reader once it answers an `IncidentCode`, so they become incident-code constants — `LOOK_ERROR_CATEGORY` → `LOOK_INCIDENT_CODE` (`crates/kinewright-app/src/inspector_ui.rs:41`) and `ROOM_TONE_ERROR_CATEGORY` → `ROOM_TONE_INCIDENT_CODE` (`crates/kinewright-app/src/timeline_ui.rs:2952`, set at `:3179` and `:3328`). AU5 §0 R134's own test moves with them and keeps every assertion it made, including its two source-text counts.

### 5.5 The card, generalised

28. **`assumed_from_present` splits into two flags, because it is not one question** (the brief critic's N7). At `d4ed8eb` it gates the revert button's `enabled` (`incident_ui.rs:198`) **and** a `details` row that prints `recovery_description(probed)` (`incident_ui.rs:234`). The signature becomes:

```rust
pub(crate) fn incident_card(incident: &Incident, revert_available: bool) -> IncidentCardView;
```

   `revert_available` keeps the button's meaning — the subject still has something to revert to — and colour supplies `asset.assumed_from.is_some()` while every other code supplies `false` until it ships a revert. The `assumed` detail row moves behind the evidence: `card_details` prints it when `incident.evidence.probed().is_some() && revert_available`, which is exactly the condition that made it true before and is now readable.

29. **`card_action_outcome` generalises against the evidence** (`incident_ui.rs:90`). Today it matches `Operation::SetAssetColorDescription` and compares against `incident.evidence.probed()`. It becomes: an action whose operation restores what the incident **captured** is a revert; any other operation is an apply; an `Explain` is explained. In code, the `Reverted` arm is guarded by `incident.evidence.probed().is_some_and(|probed| color_description == probed)`, which is `false` for every non-colour evidence and therefore reads `Applied` — correct, because no non-colour code ships an operation recovery in Part B (§1 non-goal 9).

30. **`incident_headline(code, class)` grows to 70 rows.** It is 15 rows for 12 codes today (`incident_ui.rs:113`, doc comment at `:108-112`) because the three `Rec709Compatible` codes are reachable under both classes. At 67 codes with the same three double-classed rows it is **67 + 3 = 70**, still an exhaustive match with no wildcard arm in either position. Part A's fifteen rows are unchanged, byte for byte, so IN1 §9 clause 3's pinned headline still holds. A test **must** assert every row is distinct and that the table covers the whole code set, in the shape `in1_every_headline_row_is_distinct_and_covers_the_whole_table` (`incident_ui.rs:409`) already has. **Erratum (A-R10, 2026-09-16):** distinctness and coverage are **not sufficient** as a gate, and were vacuous for 54 of the 70 rows while stage A's shim returned `explain_body(code)` for every non-colour code — core already proves those 54 bodies pairwise distinct, so the test passed for the wrong reason. The test must also discriminate a headline from a body, `incident_headline(code, class) != explain_body(code)` for every row, which fails against the shim and passes against the real table (`crates/kinewright-app/src/incident_ui.rs:758`, over the 70 rows at `:143`).

31. **The Incidents panel.** The toolbar badge becomes a button that opens an **Incidents** panel listing every open incident's card, in the shape the error-log window already has (`error_ui.rs:71-110`). Asset-subject incidents keep their Media-panel card too: `media_bin.rs:438`'s `==` filter stays, deliberately and with a comment, so the Media panel shows exactly the incidents that belong beside an asset and the Incidents panel shows all of them. **Placement is not tested** (IN1 §11.1 limit 2); the pure `incident_card` view is the tested thing, and `show_incident_card` (`incident_ui.rs:247`) is reused unchanged.

### 5.6 The written `Explain` bodies

32. **The 39 bodies Part B writes are written here**, and each is the `&'static str` core's `explain_body` returns (§3.7 rule 34) for a code with no shipped accessor. Each names what must change for the same request to succeed, and each was re-read against its own Appendix B rows for this revision (the critic's S6 and S7), which is how the actor-stopped rule of §3.2 rule 15 and the three widened bodies below were found. The other **28** rows delegate and carry **16** distinct sentences between them: the fifteen delivery, verification and QC rows each have their own, and the **thirteen `SourceColor` rows share one** — which §3.7 rule 34 names and which IN1 §13 D8 owns. One of the two core review passes **must** be briefed to read the 39 below as a person without NLE expertise would, **with Appendix B's site list beside them** (N0/Q4; the critic's S6 is the case for the second half of that sentence).

**The eleven `OpError` families.**

| code | body |
| --- | --- |
| `operation_bounds` | *This value is outside the range the edit allows. The message names the value you gave and, where it can, the range it must be in; set it inside that range and make the edit again.* |
| `operation_malformed` | *One of the fields this edit carries is missing, empty, or the wrong kind of value. The message names the field; supply a value of the kind it asks for and make the edit again.* |
| `operation_duplicate` | *Something with this identity is already in the project, so adding it again would leave two things sharing one id. Use a fresh id, or drop the second entry, and make the edit again.* |
| `operation_placement` | *This edit is valid, but not on the thing it was aimed at — a picture effect on an audio bus, a title on an audio track. Choose a target of the right kind and make the edit again.* |
| `operation_missing` | *The clip, track, asset, marker or effect this edit names is not in the project any more, so the id it used is stale. Re-read the timeline and make the edit against what is there now.* |
| `operation_structure` | *The edit is well formed, but something else has to change first — an overlap, an ordering, a live reference, or two fields that disagree. The message names what blocks it; change that, then make the edit again.* |
| `operation_relink` | *The replacement media could not be matched to the clip it is meant to replace: its fingerprint is missing, or it does not agree with the original. Relink from the media panel, or relink again with the explicit unverified-source option if you know it is the right file.* |
| `operation_unrepresentable` | *There is no value that would work: the edit cannot be expressed on this project's whole-frame grid. Choose a different frame or a different duration, rather than a different number.* |
| `operation_unknown_name` | *Kinewright does not know an effect, transition, parameter or capability by that name. Look the accepted spelling up in the capability registry and make the edit with that name.* |
| `operation_internal` | *Nothing you did caused this and nothing you can change will fix it: the part of Kinewright that applies edits has stopped, an id space has run out, or an internal time conversion overflowed. Your project is intact and still on disk. Save it if you can, restart Kinewright, and report what you were doing.* |
| `operation_color_policy` | *This colour override is not one Kinewright will write — it names a provenance the guard refuses, or supplies a field only the application may set. Change a source's colour through the incident card or the Media panel's own control rather than writing the description directly.* |

**The minted and family codes.**

| code | body |
| --- | --- |
| `lut_asset_policy` | *This LUT's hash or metadata is not the one the project recorded for it, so the file on disk is not the file the project expects. Import or restore the LUT through the look browser, which writes the store entry and the document together.* |
| `edit_revision_conflict` | *The timeline moved between the moment this edit was planned and the moment it was sent, so it was refused rather than applied to a document it was not planned against. Nothing changed; make the edit again against what the timeline shows now.* |
| `unsupported_decoder_format` | *This file's pixel format is not one the managed renderer can prove is a supported integer source surface, so it refuses to decode it rather than guess at its depth. The message names the format and the depths it found; transcode the file to an 8-, 10- or 16-bit integer format, or relink to a copy that is already in one.* |
| `media_backend_unclassified` | *The media engine refused this work and gave a reason but no code Kinewright can act on. The message is the engine's own; if it names a file, check the file is where the project expects it, then try again.* |
| `edit_plan_rejected` | *The edit plan was refused as a whole, so nothing in it was applied — either it contained no operations at all, or one of them failed. Where an operation failed the message names which one and why; fix that one, or add an operation to an empty plan, and send it again.* |
| `delivery_variant_rejected` | *The delivery variant could not be built from this project — the focal point is outside the frame, or the derived document is not valid. Adjust the focus percentages or the source timeline, then export again.* |
| `agent_branch_rejected` | *The isolated agent branch refused this request. Either the base document or the operation number named is not one it can use — read the branch's operation list and name one of the numbers it shows — or the branch's own core has stopped, in which case start a new agent thread, which builds a fresh one.* |
| `source_edit_rejected` | *Nothing was applied: something the Source edit depended on changed while the source was being verified. The message says which — if the source asset itself is gone, relink it from the media panel; otherwise reload the source, re-mark In and Out on what is loaded now, and make the edit again.* |
| `relink_rejected` | *This file cannot stand in for the asset — either it produced no verified fingerprint, or its fingerprint does not match the original. Choose a different file, or relink with the explicit unverified-source option if you know it is the right media.* |
| `project_save_failed` | *The project file could not be written, so this project is still only in memory. The message says whether it failed while serialising or while writing; free some space or choose another location, then save again.* |
| `caption_plan_rejected` | *The caption track could not be planned from these cues: there are no cues, the authored script does not line up with them, or a cue has no duration. Adjust the transcript selection or the script, then generate the captions again.* |

**The seventeen label bodies.**

| code | body |
| --- | --- |
| `operations_unclassified` | *This edit was not applied, and Kinewright has no code for the reason yet — the message is the only description it has. Read the message, change what it names, and make the edit again.* |
| `look_unclassified` | *The look or LUT could not be imported, restored or applied. The message names the file or the store; check that the project has been saved and that its LUT folder is a writable directory, then try again.* |
| `look_incomplete` | *The project was saved or opened, but not every look came with it, or the LUT folder beside it cannot be used. The message names which looks or which folder; re-import or restore them from the look browser, or save the project somewhere its LUT folder can be written, and they will be available again. The rest of the project is unaffected.* |
| `export_unclassified` | *The export did not start, or did not finish. The message names the step that stopped it; fix what it names in the export dialog and export again.* |
| `source_monitor_unclassified` | *No edit was applied: the Source monitor could not act on what is loaded. The message says which part of the source or its marks is no longer usable; reload the source, re-mark it, and make the edit again.* |
| `relink_unclassified` | *The relink could not go ahead. The message names the file or the project; choose a replacement the application can read and press Relink again.* |
| `agent_branch_unclassified` | *The isolated branch this agent thread edits in could not be created or used. The message is the only description; start a new agent thread, which builds a fresh branch.* |
| `transcript_edit_unclassified` | *The transcript edit produced no change: either the words selected contain no removable frames, or the plan behind them was refused. Select a different range of words and try again.* |
| `media_unclassified` | *Playback, import or capture stopped, or there was nothing to play. The message is the only description Kinewright has — it may name a file that has moved, or a thing the timeline still needs, such as a clip before you can press play. Do what it names, then try again.* |
| `media_incomplete` | *The project opened, but some of its media is not where the project expects it, or its timeline pictures could not be built. The message names which; relink the missing files from the media panel — the rest of the project is unaffected.* |
| `agent_unclassified` | *The agent harness could not be reached, started, or spoken to. The message names the harness; check that it is installed and on your PATH, then send the message again.* |
| `recording_unclassified` | *The recording did not start, or stopped before it finished. The message names the device or the log file; choose a camera and a microphone that are connected, then start the capture again.* |
| `project_unclassified` | *The project could not be opened, created, read, or restored from the crash-recovery journal. The message names the file; check that it exists and can be read, then open it again — if it was unsaved work that could not be restored, the last saved version of the project is still intact.* |
| `captions_unclassified` | *The captions could not be generated or saved. The message names the step; adjust the transcript selection or the output path and try again.* |
| `mixer_unclassified` | *The mixer could not do what was asked — usually because the audio it needs to learn from, or the node it was learned for, is not on the bus any more. Re-select the range or the node and try again.* |
| `media_cache_unclassified` | *The media cache could not be cleared. The message names the cache and the reason; close anything using those files, then clear it again.* |
| `timeline_unclassified` | *The timeline gesture did not complete. The message is the only description Kinewright has; re-select what the gesture needed and make it again.* |

33. **Two delegated sentences share a template without being equal** and neither is a defect: `DeliveryVerificationError::FrameCountOutOfRange` (*"Request between 1 and 16 sampled frames…"*, `delivery.rs:972-974`) and `ColorQcError::NodeBudgetExceeded` (*"Request between 1 and 16 nodes;…"*, `color_qc.rs:789-791`). §9 clause 4's distinctness test uses exact equality, so it passes; this is recorded so a later pass does not "fix" one into the other (the critic's N6).

### 5.7 The three sinks that never touched the log

34. **Three measured app-crate sinks reach the person without writing to `ErrorLog`, and Part B migrates them** (N2.5/B4). They are not in the 129, carry no source label, and are invisible to all four gate counts; they are in §1's goal sentence because they reach the status bar and the chat transcript, which that sentence claims.

| sink | what it does today | Part B |
| --- | --- | --- |
| `chat_ui.rs:631-637` (`BranchApplyOutcome::Conflict` in the branch merge) | builds *"Branch merge stopped: it was based on live revision {expected}, but live is now {actual}…"*, pushes it into the thread's transcript at `:636` and into `self.status` at `:637` | one `edit_revision_conflict` observation, subject `Agent`, evidence `Revision { expected, actual }`; the transcript entry stays, because the chat is the thread's own record |
| `chat_ui.rs:726-728` (`BranchApplyOutcome::Conflict` in the cherry-pick) | writes *"Cherry-pick stopped: expected live revision {expected}, actual {actual}"* into `self.status` and nowhere else | the same observation; `self.status` is then written by `note_incident`, not by the arm |
| `app.rs:1822` / `recovery.rs:899-904` | `self.status = crate::recovery::restore_status(result)`, whose `Err` arm is `format!("Could not restore unsaved work: {error}")`, rendered at `app.rs:1909` with no log write on the path | `restore_status` returns `Result<(), String>`'s success string as today and the `Err` arm becomes a **boxed** `project_unclassified` observation, subject `Project` (erratum **C-R40**) |

   The first two are **exactly the `edit_revision_conflict` case §4's correlation token exists for**, which is why revision 1's omission of them mattered: the contract minted a code for a conflict and then left two of its reachable sites writing a bare string. Appendix B carries all three as rows **28**, **39** and **43**, and §12's budget carries a row for them. Everything else the critic's B4 measured — the nine panel `Unavailable(String)` labels, the two `recovery.rs` modals and the LUT-store tooltips — is **D-B2**, deferred with an owner and a cost, because those are panel-local states the incident model does not yet describe. **Erratum (C-R40, 2026-09-16):** the restore sink's `Err` is `Box<IncidentObservation>`, not a bare one, as the table now reads: a `Result<String, IncidentObservation>` fails `clippy::result_large_err` at 264 bytes, which is part of the house `-D warnings` gate, so the `Err` variant is boxed (`crates/kinewright-app/src/recovery.rs:913`) and the caller writes `note_observation(*observation)` (`crates/kinewright-app/src/app.rs:2004`). Nothing else changes; the observation is still built at the producer.

---

## 6. The agent

### 6.1 `resolve_incident` verifies per code

1. `get_incidents` (`crates/kinewright-agent/src/server.rs:1319`) is **unchanged**. It serialises `log.open()`/`log.all()` wholesale; new codes, subjects and evidence flow through with no handler edit and **no input-schema change**. `GetIncidentsArgs` (`server.rs:11173`), `ResolveIncidentArgs` (`server.rs:11188`) and `ResolveIncidentOutcome` (`server.rs:11202`) are all unchanged, so the 493 B and 809 B input schemas do not move.

2. **`verify_claimed_outcome` (`server.rs:16448-16528`) becomes per code.** It is entirely colour-specific today: the irrefutable `let` at `:16456`, a `media_pool` lookup at `:16462`, and `ColorProvenance::AgentAssumption`/`assumed_from` checks at `:16481` and `:16506`. The replacement is:

```rust
fn verify_claimed_outcome(incident: &Incident, document: &Document,
                          outcome: ResolveIncidentOutcome) -> Option<CallToolResult> {
    if matches!(outcome, ResolveIncidentOutcome::Explained) { return None; }
    match (incident.code, incident.subject, incident.evidence.probed()) {
        // The one code with something to verify: the asset must carry the
        // assumption (applied) or the probed bytes (reverted).
        (IncidentCode::SourceColor(_), IncidentSubject::Asset(asset_id), Some(probed)) =>
            verify_source_colour_outcome(document, outcome, asset_id, probed),  // erratum D-R60
        // Every other code records an outcome against work this capability
        // cannot see from the document: there is nothing to verify, and a
        // refusal would be a guess (IN1 §6.3 rule 18).
        _ => None,
    }
}
```

   The three refusal codes IN1 added — `incident_not_found`, `incident_not_applied`, `incident_not_reverted` (`server.rs:16415`) — are unchanged, and Part B adds none. `incident_error` (`server.rs:16415`) is unchanged. **Erratum (D-R60, 2026-09-16):** the printed call drops its first argument, as shown above. Nothing in `verify_source_colour_outcome`'s body reads `incident` — the caller has already destructured the code, the subject and the probed description out of it, and the function's three refusal messages quote `asset_id` and the asset — and an unused parameter is `unused_variables`, which the house `-D warnings` gate turns into a build error; the signature is `(&Document, ResolveIncidentOutcome, AssetId, &ColorDescription)` (`crates/kinewright-agent/src/server.rs:16502`). **Erratum (D-R65, 2026-09-16):** hoisting `probed()` into the match key silently turns one Part A **refusal** into a record: a source-colour-coded incident carrying no probed description — reachable through `from_media_error`'s `MediaError::SourceColor(inner)` arm — on an asset that has left the media pool now records `applied` where Part A refused `incident_not_applied`. It is accepted and documented at `server.rs:16497-16512`, because with no probed description neither `applied` nor `reverted` is checkable even when the asset is present, so refusing only the departed-asset half would refuse on a fact the incident cannot act on.

3. A test **must** assert both halves: a source-colour incident resolved `applied` against an unmodified document is still refused with `incident_not_applied`, and a non-colour incident — an `operation_bounds` one on a `Clip` subject — resolves `applied` with **no** document check and no refusal.

### 6.2 The three parsers are replaced by `recovery_code()`

4. Three live agent-side consumers parse `MediaError`'s rendered text back into a code at `d4ed8eb`, each `strip_prefix("media backend error: ")` then `split_once(": ")`:

| site | function | covering test |
| --- | --- | --- |
| `crates/kinewright-agent/src/export_queue.rs:412` | the `ExportQueueError::LutStoreRootInvalid { reason }` builder — `strip_prefix` **alone**, consuming no code (erratum **D-R66**) | `export_queue.rs` `cc4_export_separates_a_refused_store_root_from_an_unsaved_project` |
| `crates/kinewright-agent/src/server.rs:13754` | `room_tone_store_error_result` | **none** — this parser has no test |
| `crates/kinewright-agent/src/server.rs:13936` | `lut_store_error_result` | `server.rs` `lut_error_fields_are_anchored_and_survive_semicolons_in_values`, `a_lut_error_value_that_begins_with_another_key_is_not_truncated` |

   All three **must** be replaced by `MediaError::recovery_code()` (`media.rs:1826`), which is the typed value they are reconstructing by hand. This requires the `LutStoreErrorCode` (11) and `RoomToneStoreErrorCode` (10) codes to reach `recovery_code()` typed — `crates/kinewright-media/src/lut_store.rs:135` and `room_tone_store.rs:188` already carry them — and that is cut item 2 (§12). `room_tone_store_error_result` **must** gain a test in the same commit, whichever way the cut falls. **Erratum (D-R66, 2026-09-16):** this rule's table describes all three parsers as `strip_prefix` then `split_once`; `export_queue.rs:412` was `strip_prefix` **alone** and consumes no code at all, so §9 clause 14's "each reads `recovery_code()`" is satisfied by the **two** code-consuming sites, with the third replaced by the one `media_refusal_payload` helper that knows the label (`export_queue.rs:438`). **Erratum (D-R67, 2026-09-16):** cut item 2 was **not** cut — N4/CR-D1 landed `MediaError::Store` (A-R12) and `media_refusal_code`'s leading-token fallback was deleted (`export_queue.rs:481`), which withdrew D-R61 — but deleting it briefly regressed one reachable served path: `From<LutParseError>` still flattened to `Backend`, so a malformed `.cube` through `import_lut_asset` served `lut_import_failed` instead of its own parse code. CR-D2 converted that impl too (`crates/kinewright-media/src/lut.rs:114-138`) and the served body is byte-identical to `b99c328`'s again, pinned on the **production** path by `in1b_a_malformed_cube_serves_its_parse_code_through_the_typed_path` (`crates/kinewright-agent/src/server.rs:29483`).

5. **The prefix itself does not move.** IN1 §9 clause 11's 750 B template (`crates/kinewright-core/src/media.rs:1711`) and the two pinned literals (`media.rs:2602`, `crates/kinewright-media/src/in1_fixtures.rs:67`) are unchanged and their five asserting sites stay green unmodified. Dropping the prefix is **D-B4** (§13).

### 6.3 `BranchError` gets a code

6. `BranchError` (`crates/kinewright-agent/src/branch.rs:46`) gains an inherent `fn incident_code(&self) -> IncidentCode` in the agent crate (§2.2 rule 8 of this contract), returning `IncidentCode::Rejection(RejectionIncident::AgentBranch)` for `CoreDisconnected`, `UnexpectedResponse`, `InvalidOperationIndex` and `DuplicateOperationIndex`, and **delegating** for `InvalidBase(OpError)` — `error.incident_code()` (§3.1 rule 6), because a rejected base document is a rejected operation and deserves the family that says so. Its evidence follows its code, by §3.4 rule 24: `Branch { reason }` for the four, the inner error's `OpError` evidence for `InvalidBase`.

### 6.4 The surface pins

7. **The served quad `7 / 5 660 / 3 510 / 998` and the registry sextuple `140 / 54 / 86 / 1 551 301 / 1 407 012 / 121 315` are asserted unmoved in all three pin sites.** Part B adds no tool, no capability and no schema field; the growth is output-only. This is the **seventeenth** consecutive measurement, and the third site is the one that carries the counter. Probe-2b T3 re-ran all three against a full correlation-token prototype and every one passed.

| pin site at `d4ed8eb` | test | what it asserts |
| --- | --- | --- |
| `crates/kinewright-agent/src/server.rs:26256` | `served_surface_is_small_and_keeps_the_internal_registry_discoverable` | the sextuple (`:26270`, `:26280`, `:26284`, `:26288`) **and** the quad (`:26298`) |
| `crates/kinewright-agent/tests/mcp_server.rs:10346` | `in1b_the_served_quad_does_not_move_for_the_seventeenth_measurement` — the renamed `in1_the_agent_surface_grows_by_two_capabilities_and_no_tool` (erratum **D-R62**) | the quad (`:10420`) plus both size constants; its message read "sixteenth" and now reads **"seventeenth"** (`:10421`) |
| `crates/kinewright-agent/tests/mcp_server.rs:2427` | `cc7_the_agent_surface_is_unchanged_by_this_slice` | the quad's components (`:2561-2564`) |

8. IN1 §6.6 rule 25 and §9 clause 13 say "both pin sites"; there are **three**. Both take erratum **R3** at promotion, and each pin site's doc comment gains one sentence naming IN1b and saying why the quad did not move: the two capabilities were already registry-only and `served_tools()` filters by `COMPACT_TOOL_NAMES` (`server.rs:726-730`), which Part B does not touch. **Erratum (D-R62, 2026-09-16):** §7 item 32 names a test whose content is exactly what the counter-carrying pin site already asserts, and adding a fourth endpoint-spinning test would make the count of pin sites four, contradicting this rule's own "all three"; the site is therefore **renamed** to item 32's name (`crates/kinewright-agent/tests/mcp_server.rs:10366`) with its IN1 assertions unchanged, and §9's regression R-A is short by it. §14 row D's citation of `:10346` and IN1 §9 clause 13's test name should follow.

9. `INSPECTOR_TOOL_NAMES.len() == 86` (`server.rs:22291`, `:26092`) and `operation_tools().len() == 54` are unchanged. The **57** `Operation` variants §3.3 rule 20's test is exhaustive over are a different count and always were.

---

## 7. Tests

Every test is an ordinary `cargo test` on both CI operating systems, needs no model, no network beyond loopback and no audio device, and follows the house `in1b_…` naming.

**core — `crates/kinewright-core/src/operation.rs`**

1. `in1b_every_op_error_variant_has_a_family` — an exhaustive `match` over all 154 variants with no wildcard, asserting every family has at least one member and the arm count is 154.
2. `in1b_time_mapping_delegates_to_its_own_seven_variants` — each `TimeMappingError` variant maps to §3.1 rule 4's family, and `OpError::TimeMapping(inner).incident_family() == inner.incident_family()`.
3. `in1b_the_two_lut_asset_variants_are_the_only_per_variant_codes` — `incident_code()` equals `IncidentCode::Operation(incident_family())` for 152 variants and `LutAssetPolicy` for the two.
4. `in1b_every_operation_names_its_incident_subject` — an exhaustive `match` over all **57** `Operation` variants with no wildcard, asserting §3.3 rule 20's precedence: the five track-addressed variants give `Track`, the three chain-addressed ones give `Chain`, `AddClip` gives `Asset`, `SetPanLaw` gives `Project`.

**core — `crates/kinewright-core/src/incident.rs`**

5. `in1b_policy_covers_every_incident_code_exactly_once_with_its_declared_severity` — replaces `in1_policy_covers_every_incident_code_exactly_once_and_every_row_blocks` (`incident.rs:1169`): `POLICY.len() == 67`, an explicit no-wildcard `ordinal()` match, every colour row `Blocks`, and every row's severity equal to §3.6 rule 31's table.
6. `in1b_every_code_delegates_its_string_to_the_accessor_that_owns_it` — accessor against accessor over all 29 delegating codes.
7. `in1b_policy_class_falls_to_explain_without_a_probed_description` — a `Rec709Compatible` row reached with `IncidentEvidence::Plain` classifies `Explain`.
8. `in1b_policy_recovery_returns_nothing_for_a_subjectless_auto_apply` — the empty-`Vec` case of §3.7 rule 33.
9. `in1b_every_explain_body_covers_the_code_set_and_the_non_colour_bodies_are_distinct` — 67 codes, no wildcard arm; the **54** non-colour bodies pairwise distinct; the **13** colour bodies each `assert_eq!`-equal to `ColorSourceError::recovery_action()`; the 15 non-colour delegating rows each equal to their own accessor's string.
10. `in1b_evidence_follows_the_code_for_every_delegating_producer` — §3.4 rule 24: `BatchError::Empty` yields `edit_plan_rejected` with `OpError { family: Malformed, op_number: None, .. }`; `BatchError::OperationFailed` and `BranchError::InvalidBase` yield the inner code with the inner family and never a `reason` string.
11. `in1b_every_subject_variant_labels_and_serialises_in_its_declared_shape` — the **eight** `label()` strings and the three-shape wire union of §3.11 rule 42, `Chain` included in both its forms (erratum **A-R13**).
12. `in1b_the_part_a_wire_body_is_byte_identical` — IN1 §6.2 rule 10's literal, unmodified (`incident.rs:1391` re-run), at 819 B.
13. `in1b_a_project_and_a_chain_subject_incident_serialise_to_their_pinned_bodies` — the second, third and, after erratum **A-R13**, **fourth** wire literals of §3.11 rule 42; the test keeps the name this item declares while pinning the `LutAsset` body too (`incident.rs:4413`).

**core — `crates/kinewright-core/src/actor.rs`**

14. `in1b_a_correlation_token_is_echoed_on_both_the_conflict_and_the_change` — §4 rule 5, both directions, plus `token: None` on `Command::Do` and every other command path.

**core — `crates/kinewright-core/src/media.rs`**

15. `in1b_matte_failures_keep_their_code_through_media_error` — `recovery_code()` is `Some` for both new variants and the rendered text carries the same code token it did as a `Backend` string.

**app — one per migrated label, 17 of them**

16. `in1b_<label>_produces_its_declared_code_class_and_severity` for each of the 17 labels of §2.1, each driving the label's own representative site through `pending_observations` into the log and asserting the code, subject, class and severity Appendix B declares. `"Timeline"` is one of the 17 and is driven through `InspectorEdits::incident_code()` (§5.4 rule 27). Every one of the 17 is writable: after §3.3 every subject Appendix B names is constructible, and after §3.3 rule 20 the `Operations` and `Agent branch` rows have a declared subject to assert rather than a disjunction. **Erratum (C-R46, 2026-09-16):** **four** of the seventeen prove the declaration rather than the trigger — `Relink` (row 92, inside a modal `egui::Window` closure), `Agent branch` (row 48) and `Agent` (row 34), both needing a spawned harness and a live MCP endpoint, and `Branch preview` (row 46), needing a branch whose thumbnail renderer refuses — and each carries an `include_str!` source-shape assertion over its module's production half instead. The other thirteen drive the real arm, including `Recording`, `Media cache` and `Media`, whose blockers were module privacy and nothing at all; §10 records the residue as a limit.

**app — the sink and the card**

17. `in1b_the_sink_gate_counts_are_zero_zero_one_one` — §5.3 rule 20's four greps, as a test over the crate's own sources. The mechanism is stated so the test cannot pass vacuously: it walks `concat!(env!("CARGO_MANIFEST_DIR"), "/src")` recursively, **asserts it read at least 20 files**, and applies each of the four patterns with the same definition exclusions §5.3 rule 20 spells. Counts 1 and 2 are `== 0` and would otherwise pass on an empty read; the file-count assertion and count 4's `== 1` are what make the bundle honest (the critic's N5).
18. `in1b_note_incident_writes_the_status_and_opens_the_window` — §5.3 rule 19.
19. `in1b_the_badge_counts_open_incidents_not_log_lines` — the rewritten `in1_the_open_count_and_the_audit_line_are_different_quantities` (`app.rs:4694`), asserting the badge's reader.
20. `in1b_a_conflict_is_matched_to_the_refused_send_by_token` — the rewritten `in1_a_conflict_is_matched_to_the_refused_send_not_the_landed_one` (`app.rs:4603`). It asserts the identity match; it does **not** discriminate against the two-tier matcher and §4 rule 8 says so.
21. `in1b_a_foreign_conflict_leaves_every_router_send_outstanding` — §4 rule 7's "do nothing" case. **This is the test that fails at `d4ed8eb`**, because the `.or_else(…)` fallback at `app.rs:1147` consumes a send.
22. `in1b_one_open_with_n_missing_files_opens_one_incident` — the aggregate shape of §5.1 rule 14.
23. `in1b_n_visual_cache_failures_open_n_asset_incidents` — the per-item shape of the same rule.
24. `in1b_a_branch_conflict_opens_an_edit_revision_conflict_incident` — §5.7's two chat sinks, both arms.
25. `in1b_every_headline_row_is_distinct_and_covers_the_whole_table` — 70 rows (the existing `incident_ui.rs:409` test, regrown).
26. `in1b_a_non_colour_card_offers_the_explain_body_and_nothing_to_press` — `revert_available` false, one disabled action, the written body.
27. `in1b_the_assumed_detail_row_needs_both_a_probe_and_a_revert` — §5.5 rule 28's split flag.

**agent**

28. `in1b_a_non_colour_incident_resolves_without_a_document_check` — §6.1 rule 3.
29. `in1b_a_source_colour_claim_is_still_verified` — the other half of the same rule.
30. `in1b_the_lut_store_result_reads_its_code_from_the_typed_error` — `lut_store_error_result` with no `strip_prefix`.
31. `in1b_the_room_tone_result_reads_its_code_from_the_typed_error` — the parser that has no test today.
32. `in1b_the_served_quad_does_not_move_for_the_seventeenth_measurement` — the three pin sites of §6.4.
33. `in1b_every_code_fits_the_measured_ceiling` — the rewritten `in1_the_fixture_incident_is_pinned_and_every_code_fits_the_ceiling` (`server.rs:~26140`), quantified over **67** codes × **eight** subject shapes (536 pairs, erratum **A-R13**), with the fixture pinned at 819 B and the ceiling at 2 048 B.

---

## 8. Telemetry

`IncidentTelemetry` and `QuestionKind` are **unchanged** from IN1 §8. Part B adds no field, changes no `skip_serializing_if`, and reports no new category. Two notes the rules require:

1. `tool_calls` is still incremented only by `resolve_incident` (`server.rs:1385`), and `resolved_after` is still stamped by `IncidentLog::resolve` from the session clock it owns (IN1 §5.2 rule 12's erratum). The router's new sends do not stamp telemetry.
2. Part B multiplies the number of incidents in a session by roughly the number of labels, so `"telemetry":{"tool_calls":0}` is now the overwhelmingly common serialisation. IN1 §6.2 rule 12's `skip_serializing_if` decision is what keeps that from costing eight keys of `null` per incident, and §3.11 rule 43's ceiling measurement is taken on the skipped shape.

---

## 9. Exit gate

Every numbered clause is falsifiable, is discharged by an ordinary `cargo test` on **both** CI operating systems, and needs no model, no network beyond loopback and no audio device. **A clause that can pass at `d4ed8eb` before IN1b exists is not a clause** — each clause below names the rule it discharges, the test that fails today, and the **measured** reason it fails.

| # | clause | discharges | test | fails at `d4ed8eb` because, measured |
| ---: | --- | --- | --- | --- |
| 1 | `OpError::incident_family()` is an exhaustive match over **154** variants with no wildcard, every family non-empty, and `TimeMapping` delegating to seven sub-variants | §3.1 rules 2–4 | items 1–2 | the function does not exist: `grep -c 'fn incident_family' crates/kinewright-core/src` = 0 |
| 2 | `POLICY.len() == 67`, the explicit `ordinal` match covers it with no wildcard, every colour row is `Blocks`, and every row's severity equals §3.6 rule 31's table | §3.2 rule 12, §3.6 rules 29–31 | item 5 | `POLICY.len() == 12` (`incident.rs:1170`) |
| 3 | `policy_class(code, &IncidentEvidence)` classifies an incident carrying no probed description, and a `Rec709Compatible` row without one falls to `Explain` | §3.5 rule 26 | item 7 | the signature is `(IncidentCode, &ColorDescription)` (`incident.rs:680`) |
| 4 | Every one of the **67** codes reaches an `Explain` body through a core exhaustive match with **no wildcard**; the **54** non-colour bodies are pairwise **distinct** (39 written in §5.6, 15 delegated); the **13** colour bodies are each equal to `ColorSourceError::recovery_action()` | §3.7 rule 34, §5.6 | item 9 | `explain_body` does not exist, and `ColorSourceError::recovery_action()` (`color.rs:328-332`) is a `const fn` with no `match` returning one string for all thirteen — which is why this clause does not assert 67 distinct bodies (N2.5/B1) |
| 5 | All **eight** `IncidentSubject` variants label and serialise in their declared shapes, including `Chain`'s nested union and the three bare strings (erratum **A-R13**) | §3.3, §3.11 rule 42 | item 11 | seven of the eight do not exist (`incident.rs:267` declares `Asset` alone) |
| 6 | The sink gate reads **0 / 0 / 1 / 1** in §5.3 rule 20's four spelled greps at the final app commit, and after routing one fixture incident `app.status` is that incident's headline and `app.error_log_open` is `true` | §5.3 rules 17–21 | items 17–18 | the four greps read **126 / 3 / 1 / 0**, re-measured for this revision with the exact commands rule 20 prints |
| 7 | The toolbar badge renders `IncidentLog::open_count()`, and the two-counts test asserts the **badge's** number as well as the log's | §5.4 rules 25–26 | item 19 | the badge reads `self.error_log.len()` (`app.rs:1875`, `:1879`) |
| 8 | **One per-label test per migrated label, 17 of them**, each asserting the code, subject, class and severity Appendix B declares; **four** of the seventeen assert the declaration and the site's source shape rather than driving the arm (erratum **C-R46**) | §2.1 rule 3, §3.3 rule 20, §3.4 rule 24, §7 item 16 | the 17 `in1b_<label>_…` tests, over items 4 and 10 | none exists; `"Timeline"` has no test of any kind. Every label is now writable: §3.3 makes all seven subjects constructible and rule 20 supplies the `Operations`/`Agent branch` subject the tree previously discarded at `app.rs:1578-1589` |
| 9 | `Command::DoIfRevision` carries an optional correlation token that the actor echoes on **both** `Event::RevisionConflict` and `Event::DocumentChanged` | §4 rules 2–5 | item 14 | no field on either type (`actor.rs:57-61`, `:113-116`) |
| 10 | `reconcile_router_sends` matches a conflict to the refused send **by token**, and a conflict with no matching token leaves every outstanding send in place | §4 rules 7–8 | items 20–21 | the `.or_else(…)` fallback at `app.rs:1147` consumes a foreign conflict's send; item 21 is the half that discriminates and item 20 is not (§4 rule 8) |
| 11 | The three aggregate bypasses produce their declared shapes: one incident for one open with *n* missing files, *n* incidents for *n* visual-cache failures | §5.1 rule 14 | items 22–23 | all three are direct `error_log.push` lines (`app.rs:760`, `:775`, `:1377`) that never set `error_log_open` |
| 12 | The three log-bypassing sinks open incidents: both `BranchApplyOutcome::Conflict` arms open an `edit_revision_conflict`, and a failed restore opens a `project_unclassified` | §5.7 rule 34 | item 24 | `chat_ui.rs:631-637` and `:726-728` write `self.status` and the transcript only; `app.rs:1822` writes `self.status` only |
| 13 | `verify_claimed_outcome` verifies a source-colour claim and records every other code without a document check | §6.1 rules 2–3 | items 28–29 | the function's first statement is an irrefutable `let` on `IncidentSubject::Asset` (`server.rs:16456`) |
| 14 | The three `strip_prefix("media backend error: ")` parsers are gone; the **two** code-consuming sites read `MediaError::recovery_code()` and the third consumes only the payload, `room_tone_store_error_result` included (errata **D-R66**, **D-R67**) | §6.2 rule 4 | items 30–31 | three parsers exist and `room_tone_store_error_result` is untested |
| 15 | `MediaError::recovery_code()` is `Some` for **9 of 14** variants (erratum **A-R12**'s `Store`; 8 of 13 before it) and both matte codes survive the `From` impls typed | §3.9 rule 36 | item 15 | it is `Some` for 6 of 11 (`media.rs:1826`) and both flatten into `Backend(String)` |
| 16 | **The token budget, two assertions that can fail**: `assert_eq!(serde_json::to_vec(&incident).len(), 819)` for the named fixture incident, and `<= 2_048` for all **67** codes across all **eight** subject shapes — 536 pairs, erratum **A-R13** | §3.11 rule 43 | item 33 | the loop quantifies over 12 codes and one subject (`server.rs:26169`), and the ceiling constant is 1 024, which the measured worst of 1 354 B exceeds |
| 17 | The served quad is `7 / 5 660 / 3 510 / 998` and the registry sextuple is `140 / 54 / 86 / 1 551 301 / 1 407 012 / 121 315`, in **all three** pin sites, with the counter reading "seventeenth" | §6.4 rules 7–8 | item 32 | **by a word, and the clause says so**: the quad's values already hold at `d4ed8eb`; what fails is `tests/mcp_server.rs:10401`, which reads "sixteenth", and the third pin site's absence from IN1 §6.6 rule 25's "both" |
| 18 | `incident_headline` is **70** rows over an exhaustive match in both positions, every row distinct | §5.5 rule 30 | item 25 | it is 15 rows and its first statement is an irrefutable `let` on `IncidentCode` (`incident_ui.rs:114`) |

**Regressions guarded and process gates.** None is a clause: each passes at `d4ed8eb` by construction, or is not a `cargo test`.

- **R-A — IN1's full suite stays green**, with the named exceptions of §14's erratum list: `in1_policy_covers_every_incident_code_exactly_once_and_every_row_blocks`, `in1_policy_rows_are_declared_in_table_order`, `in1_the_open_count_and_the_audit_line_are_different_quantities`, `in1_a_conflict_is_matched_to_the_refused_send_not_the_landed_one`, `in1_every_headline_row_is_distinct_and_covers_the_whole_table`, `in1_source_colour_incidents_mirror_the_classifier_one_to_one`, `in1_from_media_error_maps_only_the_asset_scoped_source_colour_refusal` and `in1_the_fixture_incident_is_pinned_and_every_code_fits_the_ceiling` are **rewritten**; every other `in1_` test passes **unmodified**, which probe-2b T3 confirmed for all 15 app `in1_` tests against a full token prototype. **Erratum (A-R7, 2026-09-16):** this list is short by **four**, each change mechanical and each forced by a signature or a count this contract itself mandates: `in1_every_opened_incident_carries_its_codes_field_and_declared_severity` (`crates/kinewright-core/src/incident.rs:2810`) built each row's observation from `entry.code.source_error()`, which exists only for a colour code, and now `continue`s on a non-colour row; `in1_every_auto_apply_row_applies_cleanly_and_a_bt2020_probe_explains_instead` (`incident.rs:2870`) now passes the evidence (§3.5 rule 26); `in1_every_headline_row_is_distinct_and_covers_the_whole_table` (`crates/kinewright-app/src/incident_ui.rs:435`) asserted 15 rows and now asserts **70**; and the agent test R-A *does* name needed one change R-A did not anticipate, its `in1_every_source_error` fixture growing from twelve classifier variants to thirteen (IN1b-R4). **Erratum (D-R62, 2026-09-16):** `in1_the_agent_surface_grows_by_two_capabilities_and_no_tool` is a **fifth**, and is a rename rather than a rewrite — §6.4 rule 7 requires its message to read "seventeenth" and §3.11 rule 43 its ceiling assertion to read `2_048`, so it cannot pass unmodified; see §6.4 rule 8.
- **R-B — the 750 B template does not move.** `IN1_MANAGED_DECODE_REFUSAL` (`crates/kinewright-core/src/media.rs:2602` (test-local), asserted at `:2697` and `:2699`) and its production twin (`crates/kinewright-media/src/in1_fixtures.rs:67`, asserted at `:298`, `:299`, `:304`) are unmodified and green.
- **R-C — the Part A wire literal does not move.** IN1 §6.2 rule 10's body is byte-identical after Part B (§3.11 rule 41, proved by probe-2b T2 at 819 B), asserted by `in1_the_fixture_incident_serialises_to_the_pinned_wire_body` (`incident.rs:1391`) **unmodified**.
- **R-D — `rec709_compatible` is untouched** and IN1 §9 clause 19's equivalence test (`incident.rs:1684`) passes unmodified.
- **R-E — neither managed-colour allowlist moves**, and the `"application_default or user_override"` literals at `crates/kinewright-core/src/delivery.rs:2211` and `crates/kinewright-media/src/export.rs:2677` are unmodified and green (IN1 §9 regression R2).
- **P1 — the build gates.** `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`, and **`cargo check -p kinewright-app`** before **each** commit. Every cargo command runs after `source ./scripts/setup-ffmpeg.sh`. P1 is the reason §5.3 rule 21's shim exists. **Erratum (D-R63, 2026-09-16):** `cargo fmt --all` is **not** a per-implementer command while §14's `A → (C ‖ D)` order has two implementers live — it reformats the other implementer's in-flight files and can invalidate an exact-string edit mid-way through, and it did so once from the agent tree. The per-crate spelling (`cargo fmt -p <crate> -- --check`) is the one to use while two implementers are live, with the workspace `--check` run once before the commit.
- **P2 — two review passes per crate** before each commit, per the slice recipe. One of the two core passes **must** be briefed to read §5.6's bodies as a person without NLE expertise would, with Appendix B's site list beside them (N0/Q4).
- **P3 — the promotion note records [probe-2c]'s outcome** (§10), so the contract in `docs/` carries numbers and not markers.

---

## 10. Measurement limits, and what the implementer still measures

**Limits IN1b records rather than papers over.**

1. **The card's placement is not proven at all.** There is no `crates/kinewright-app/tests/`, `KinewrightApp::new` is private (`app.rs:270`), and the Incidents panel is drawn by `show_incident_card` (`incident_ui.rs:247`), which is untested by design. IN1 §11.1 limit 2 is unchanged and now covers a second surface.
2. **Severity is derived from control flow by reading, not by running.** Appendix B's severity column was assigned by reading each of the 132 rows' sites at `d4ed8eb`. No test can prove that a site which returns early "blocks"; what §9 clause 2 proves is that the table and the code agree, and what the per-label tests prove is that each label's representative site produces the declared value.
3. **The `ErrorLog` text-assertion risk is zero and was measured.** Probe-1b P8: **four** test functions, **seven** assertions, all `count_with_source("Incident")`, none on message text, in the **one** app test module that constructs a `KinewrightApp` (`app.rs:2955`'s `mod tests`, through `in1_harness` at `app.rs:4132`). The design brief's R5 is withdrawn (N1.5 §11).
4. **The 129 is an app-crate `record_error` census by construction.** It does not and cannot see the agent crate's refusal population (§13 D-B1), the panel-local sinks (§13 D-B2), the four `Result<_, String>` seams (§13 D-B3) or the matte proof and coverage errors of §0.2/e. §5.7's three rows are in Appendix B because they were found by hand, not by the census, and nothing claims the hand search was exhaustive.
5. **The 1 354 B worst case is a measurement, not a bound.** `observed` is not bounded by the type system (§3.11 rule 44).
6. **`operation_internal` now answers for two different things** — an actor that stopped and an id space that ran out — and its body names both. That is one sentence doing two jobs honestly rather than two codes; if IN3 finds a person confused by it, minting `actor_stopped` is a one-row change and 14 rows move with it.
7. **No IN1b number is per-OS.** If one is ever found that differs, it becomes a doc-comment note beside the single constant, never a second constant.
8. **Cutting item 1 removes the only surface non-asset incidents have.** §12 says so where the cut is listed.

**The one remaining measurement.** Revision 1 carried four `[probe-2b]` markers; probe-2b measured all four and §§3.2, 3.11, 5.3 and 13 carry the numbers. One measurement was owed against the implementation; it was taken on 2026-09-16 and §3.11 rule 43 carries it, so this table records what was expected beside what was measured.

| # | figure | what [probe-2c] measured | expected outcome, and the measurement |
| ---: | --- | --- | --- |
| 1 | `IN1_INCIDENT_SERIALIZED_CEILING_BYTES` | the worst serialised incident over **67 codes × the seven subject shapes of §3.3 rule 17** (469 pairs), re-measured because N2.5/B2 and B3 changed the subject shapes after probe-2b measured 402 pairs at 1 354 B | **unchanged at 2 048.** Three of the old shapes became unit variants and are strictly smaller than the `ExportJob(u64::MAX)` that produced the old worst; the one new shape, `Chain(AudioChain::Bus(AudioBusId(u64::MAX)))`, renders `{"chain":{"bus":18446744073709551615}}` — **3 B** more than `{"export_job":18446744073709551615}` — so the worst is expected at **≈1 357 B**, still above 2 048 / 2 and far below 2 048. If the measurement contradicts this, the ceiling is re-set to the smallest power of two above the measured worst and `server.rs:26186`'s divisor is reconsidered in the same commit; `IN1_INCIDENT_SERIALIZED_BYTES` = **819** does not move either way, because §3.11 rule 41's byte-identity is structural. **Measured 2026-09-16:** over **536** pairs — 67 codes × the **eight** shapes of §3.3 rule 17 as amended by erratum A-R13 — the worst is **1 351 B** in core and **1 358 B** in the agent's independently written loop, on `unsupported_decoder_format` / `Chain(Bus(AudioBusId(u64::MAX)))` in both; the 7 B is review-2 N-3's real 77 B `ColorQcError::NodeRemovalRejected` `allowed` literal against core's 70 B synthetic. **The ceiling stays 2 048**, `worst > CEILING / 2` holds unmodified, 819 did not move, and `LutAsset(LutAssetId(u64::MAX))` is 3 B narrower than the worst pair, so the eighth subject is not the widest |

---

## 11. The hand-run checklist

Nothing below is a `cargo test`. There is one person path per subject kind that a person can provoke, because §10 limit 1 means no automated test sees a card on screen.

1. **`Asset` — the smoke session, repeated.** Import an untagged WebM, press play, confirm the card appears beside the asset in the Media panel **and** in the new Incidents panel, and that **Revert to probed description** still restores the probed line. This is IN1's own session and it must not have regressed.
2. **`Clip` — a refused trim.** Trim a clip past its source end, confirm the badge increments, open the Incidents panel from the badge, and confirm the card reads `operation_bounds`' written sentence with the clip named in `subject_label` and nothing to press.
3. **`Chain` — a refused mix on a bus.** Learn a noise profile with no silence selected on a bus effect, and confirm `mixer_unclassified`'s sentence appears against **`Bus n`** rather than in a scrolling window. (Revision 1 wrote `Track n` here; the three Mixer sites are `AudioChain`-scoped and no `TrackId` is in scope at any of them — N2.5/B3.)
4. **`ExportJob` — a refused export.** Export with no output path chosen, confirm one `export_unclassified` incident against `Export`, then fix the path and confirm the badge returns to zero when the incident resolves.
5. **`Project` — an open with missing media.** Open a project whose media has been moved, and confirm **one** `media_incomplete` card listing every missing file, that the Incidents surface **opens** where it did not before (§5.1 rule 14), that its severity reads as a degraded result rather than a blocked one, and that the project is usable.
6. **`Agent` — a branch conflict.** Start an agent thread, let it build a branch, edit the live timeline by hand, then merge the branch: confirm the conflict now opens an `edit_revision_conflict` incident with the card's sentence, as well as the chat transcript line it already writes (§5.7).
7. **The audit view still works.** With several incidents open, open the error-log window and confirm it lists one `"Incident"` line per opened incident and that its count and the badge's count are different numbers — the visible half of §5.4 rule 25.

**`Track` has no hand-run item, and the reason is stated rather than omitted.** Its one Appendix B row, 124 (`timeline_ui.rs:3266`), fires when `std::thread::Builder::spawn` fails for the room-tone worker, which cannot be provoked from the user interface. The per-label test (§7 item 16, the `Media` label) is the only evidence that row produces its declared subject.

---

## 12. Line budget, and what is cut first

**Calibration, stated because it is the reason this budget is not IN1 §12's.** IN1 §12 estimated Part A at 4 039–5 279 insertions. Part A landed **6 758** insertions across 70 files (`git diff --stat e3ad623 29fad67`) — **+28 %** over the upper bound — and that split **6 749 code / 9 docs**, because the contract landed in the previous commit. **The calibration is therefore a code-only calibration and this table carries no docs row**; the companion contract lands in its own promotion commit, as IN1's did at `e3ad623`.

| deliverable | crate | estimate |
| --- | --- | ---: |
| `IncidentFamily`, the grouped 154-arm `incident_family()`, `TimeMappingError::incident_family()`, `incident_code()`, the one `#[allow]`, the three coverage tests | core (`operation.rs`, `time.rs`, `lib.rs`) | 140–200 |
| **`Operation::incident_subject()`** over 57 variants with its precedence, the two drain arms that stop discarding `op`/`operations`, the exhaustive test | core + app | **80–130** |
| `IncidentCode` 12 → 67 with delegating `code()`/`field()`, `POLICY`'s 67 rows with severities, the exhaustiveness and ordering tests | core (`incident.rs`) | 520–700 |
| the **39** written `Explain` bodies, the 28 delegating arms, and the **70** headline rows | core + app | 260–380 |
| six `IncidentSubject` variants, `AudioChain`'s three derives, `label()`, the `IncidentEvidence` variants, `probed() -> Option`, `policy_class(&IncidentEvidence)`, subject-generic `policy_recovery`, the four observation constructors | core (`incident.rs`, `media.rs`) | 300–420 |
| two `MediaError` passthrough variants, two `recovery_code()` arms, two `from_media_error` arms made total, the media rendering test | core | 60–100 |
| `CaptionPlanError` typed through the app wrapper, with the two id-exhaustion variants carved out | app + core | 30–60 |
| **the correlation token**: one field on three `actor.rs` types, the actor's echo, the eight breaking arms, the two `media_workflow.rs` senders, `RouterConflict` losing `expected`, the identity matcher replacing the two-tier match, one test | core + app + agent | **172 (measured, probe-2b T3: 172 insertions / 73 deletions, 38 sites, 7 files, 51 hunks)** |
| **the 129 call-site rewrites**, the per-source observation builders, ~20 new `use` lines, `pending_observations` `pub(crate)`, and the `Result<_, String>` helpers that are re-typed | app | **1 160–2 190** (measured centre ≈1 650) |
| **the three log-bypassing sinks** of §5.7 and `restore_status`'s new shape | app | **40–80** |
| the sink gate, `note_incident`, `record_error` deleted, the badge | app | 150–250 |
| `revert_available`, `card_action_outcome` generalised, the Incidents panel | app | 300–450 |
| the two dynamic producers typed | app | 40–80 |
| the four out-of-core enums given `incident_code()`/`incident_observation()` (`SourceEditRejection` 11, `RelinkRejection` 2 + the conflict struct, `ProjectSaveError` 2, `BranchError` 5) | app + agent | 150–250 |
| `verify_claimed_outcome` per code, the three parsers replaced, the two constants, the three pins re-asserted | agent | 340–580 |
| the 17 per-label tests and the rest of §7 | core + app + agent | 650–950 |
| | **total** | **4 392–6 992** |

With Part A's measured **+28 %**, **plan for 5 600–8 900 insertions** and say so here rather than discovering it (§0.3 D1). Probe-1b's P5 calibration is the load-bearing row: a migrated site costs **9–25 lines, median 13**, measured over five hand-migrated sites of five different shapes, and the shared core plumbing measured **531 insertions** before any body was written.

**Cut order.** Four things are cuttable, each because no §9 clause rests on it.

1. **The Incidents panel** (§5.5 rule 31, ~150 of the 300–450 card row). Cut **first**: placement is untested by contract (§10 limit 1), so nothing can fail on it, and asset-subject incidents keep their Media-panel card. **If cut, a non-asset incident has no surface of its own at all**: `media_bin.rs:438`'s `==` filter keeps the Media panel asset-only and the audit window shows a headline line and nothing else, so §11's hand-run items 2–7 become the only evidence those incidents reach anyone. §10 limit 8 records it.
2. **The three `strip_prefix` parsers together with the per-variant `LutStoreErrorCode` (11) and `RoomToneStoreErrorCode` (10) codes** (§6.2, ~150–250). **They are one item** (N1.5 §10): the parsers reconstruct exactly those codes, so cutting the codes makes replacing the parsers impossible. Cut **second**. If cut, §9 clause 14 is struck from the gate before promotion and `room_tone_store_error_result` still gains its missing test.
3. **The two dynamic producers typed** (§5.4 rule 27, 40–80). Cut **third**: IN1 §10 rule 5 permits naming them as `Explain` placeholders instead, and the sink gate still reads 0/0/1/1. If cut, the `inspector_ui.rs:793` *n*-log-lines bug survives and §10 records it.
4. **`CaptionPlanError`'s typing** (§3.10, 30–60). Cut **fourth**: the `Captions` label keeps its placeholder and loses one code. This is the cheapest item on the list and the last one worth cutting.

**Never cut**, because they are the exit gate: the sink gate's four counts; `POLICY` exhaustive over 67 codes by explicit match with its severity column; `incident_family()`'s exhaustive match over 154 variants; `Operation::incident_subject()`'s exhaustive match over 57; the **39** written `Explain` bodies (the 15 delegated-distinct and the 13 sharing one sentence come free with their accessors); the badge on `open_count()`; the 17 per-label tests; §5.7's three sinks; the correlation token and its identity matcher; the two serialised-size constants over 67 × 7; the served-quad and registry pins in all three sites; `cargo check -p kinewright-app`.

---

## 13. Explicit deferrals

Each names why it is a slice and not a flag, and carries a cost and an owner.

- **D-B1 — the agent crate's refusal population.** `crates/kinewright-agent/src/server.rs` carries **277** `error_text(`/`error_structured(` call sites at `d4ed8eb` — 252 + 25, definitions excluded, measured by probe-2b T7 — each reaching a person through the chat panel with a sentence and no code, no class and no recovery. Only **22** of the 25 structured calls carry a `code` key: **7.9 %** of the population is typed. That is 2.2× the app's 126 `record_error` call sites and it is the surface IN1's closing sentence is written about. **Not free:** it is a second migration with its own code set, its own class decisions and its own gate, and it wants the investigator surface to read the result — unifying `error_structured`'s existing `code`/`field`/`observed`/`allowed`/`recovery_action` shape with `IncidentCode` is the actual work. **Cost:** of the order of the app migration, 1 500–2 500 insertions. **Owner: IN3**, with the investigator surface. §5.3 rule 22 states why the sink gate's four counts do not claim them.
- **D-B2 — the app's panel-local untyped sinks.** Measured inventory at `d4ed8eb`, in three groups. **(a) Nine `Unavailable(String)`-shaped panel labels** rendering a flattened string straight to the person: `color_scopes_ui.rs:810`, `color_qc_ui.rs:1200-1203`, `preview_ui.rs:1138-1143` (matte view), `preview_ui.rs:920-925` (QC mask), `transcript_ui.rs:162-166`, `chat_ui.rs:1168-1172`, `export_ui.rs:1331-1341`, `:569-575`, `:953-959`. None of `color_scopes_ui.rs`, `color_qc_ui.rs`, `matte_overlay_ui.rs` or `transcript_ui.rs` contains a single `record_error(`. **(b) Two modal dialogs outside `KinewrightApp` entirely**: `recovery.rs:696` `egui::Window::new("Crash recovery unavailable")` with `ui.label(message)` at `:700`, fed from an `Arc<Mutex<Option<String>>>` written at `recovery.rs:506`, `:612`, `:764`; and `recovery.rs:636` `"Recover unsaved work?"` with `damage_description(damage)` at `:661`/`:672`. **(c) Disabled-control tooltips** carrying the LUT-store refusal string: `app.rs:1750-1757`, `look_browser_ui.rs:255-259`, `inspector_ui.rs:1659`, `:2028`, from `project.rs:43`/`:230`'s `lut_store_error: Option<String>`. **Not free:** these are *panel-local states*, not events — a label that says "scope unavailable" is a property of the panel's current frame, and an `Incident` is a thing that happened at a revision and stays open until resolved. Giving them incidents means either an incident whose state the panel owns, or a second model; that choice belongs with IN2's persistence work, which already re-shapes the observation. **Cost:** 300–500 insertions for (a), 80–150 for (b), 40–80 for (c). **Owner: IN2.** §1 non-goal 2 names them and §1's goal sentence excludes them by surface. **Owner line added at IN2 Part A's promotion (2026-09-16):** **Owner: IN2 Part B** (`IN2B-INCIDENT-PERSISTENCE.md`), with **(a) the first cut** — re-located by IN2 probe-1 P9 at `7e85972`, where group (c)'s LUT-store tooltips are now **8** consumers rather than 4 ([IN2](IN2-INVESTIGATOR-SESSIONS.md) §13). **Owner line added at IN2 Part B's promotion (2026-09-22):** **Discharged by IN2 Part B** ([IN2B-INCIDENT-PERSISTENCE.md](IN2B-INCIDENT-PERSISTENCE.md) §7) — (a) first cut through pure `panel_observation` fns at the state transition, (b) both modals on damage only, and **(c) discharged, not built**, as already covered by rows 3/9 (IN2B §7 rule 8).
- **D-B3 — the four `Result<_, String>` seams that hide a typed code.** `MatteProofSource::matte_proof(...) -> Result<MatteProof, String>` (`matte_overlay_ui.rs:100`) and `matte_coverage_statistics(&coverage).map_err(|error| error.to_string())` (`color_scopes_ui.rs:1073`) flatten `MatteProofError`'s five and `MatteCoverageError`'s five codes — this is the seam §0.2/e's ruling turns on. `ColorQcSource::measure_with_nodes(...) -> Result<_, String>` (`color_qc_ui.rs:213`), stored in the panel's own `self.error` (`color_qc_ui.rs:958`), flattens `ColorQcError`'s six. `worker_verification`'s `Err` becoming `ExportVerification::Unavailable(String)` (`export_ui.rs:1044`, from `export_ui.rs:1722`) flattens `DeliveryVerificationError`'s five. **These four seams are why eleven of the 67 declared codes are unreachable from the 129** (§3.2 rule 13): the `DeliveryVerification` and `ColorQc` codes are declared so `IncidentCode::code()`'s delegation stays total, and they will become reachable the day these seams are typed. **Not free:** each seam crosses a trait boundary the panels implement, so the change is a trait signature plus every implementor plus the panel state that stores the result. **Cost:** 250–450 insertions. **Owner: IN2.** **Owner line added at IN2 Part A's promotion (2026-09-16):** **Owner: IN2 Part B** (`IN2B-INCIDENT-PERSISTENCE.md`) — IN2 probe-1 P9 counts **six** method signatures rather than four, flattening **21** typed error values and keeping **11** of the 67 declared codes unreachable, so Part B's exit gate must assert its session count over the newly reachable population ([IN2](IN2-INVESTIGATOR-SESSIONS.md) §13, §3.1 rule 1). **Erratum (E-B1, 2026-09-22):** the 11 were already allowlisted before Part B, so the session count replaces the never-existent explicit edit ([IN2B](IN2B-INCIDENT-PERSISTENCE.md) §0.5 E-B1, §12 clause 6). **Owner line added at IN2 Part B's promotion (2026-09-22):** **Discharged by IN2 Part B** ([IN2B-INCIDENT-PERSISTENCE.md](IN2B-INCIDENT-PERSISTENCE.md) §6) — six typed seams plus the eighth (`export_ui.rs:62`), with 74/0 reachability gated.
- **D-B4 — the vestigial `media backend error: ` prefix.** IN1 §13 D16 called it "a visible-text change"; it is also parsed by three live consumers (§6.2 rule 4), and Part B removes that reason by handing them `recovery_code()`. What remains is the byte pins: IN1 §9 clause 11's 750 B template and the two literals at `media.rs:2602` and `in1_fixtures.rs:67`. **Cost:** one `#[error]` attribute, one template re-capture and five asserting sites — 40–80 insertions. **Owner: IN2**, with the persistence work that already re-shapes the observation. IN1 §13 D16's owner line changes from "IN1 Part B" to "IN2". **Owner line added and cost corrected at IN2 Part A's promotion (2026-09-16):** **Owner: IN2 Part B** (`IN2B-INCIDENT-PERSISTENCE.md`); the **cost is 90–160, not 40–80**, because IN2 probe-1 P9 measured the blast radius at **12** asserting sites rather than five, including **one production consumer** that still parses the prefix (`export_queue.rs:451`'s `strip_prefix`) ([IN2](IN2-INVESTIGATOR-SESSIONS.md) §13). **Owner line added at IN2 Part B's promotion (2026-09-22):** **Discharged by IN2 Part B** ([IN2B-INCIDENT-PERSISTENCE.md](IN2B-INCIDENT-PERSISTENCE.md) §8) — lands whole in one commit (template, asserts, parser deletion, 708 B re-capture), re-measured at **14 + 1** by Part B probe-1.
- **D-B5 — the three `reason: String` `OpError` variants and the seven `Result<_, String>` app helpers.** `InvalidTrackAutomation` (`operation.rs:982`), `InvalidClipGainEnvelope` (`:979`) and `InvalidEffectAutomation` (`:924`) carry a stringified `curve.validate()` error; seven app helpers (§3.10 rule 40) compose their own sentences from literals. After Part B the person still reads a stringified error in those cases — in core, not in the app. **Not free:** the curve validator needs a public typed error and the seven helpers need seven small enums, each of which is a code decision. **Cost:** 200–400 insertions. **Owner: IN3**, with the per-code bodies of IN1 §13 D8.
- **D14 — multi-project incident attribution, re-deferred** (N0/Q8). `media_events` is a single app-level receiver from one engine (`app.rs:133`), so a playback incident is attributed to the focused project (IN1 §5.2 rule 5). The measured cheaper route is a project tag on `MediaEvent` — 3 variants (`media.rs:1420-1424`), 13 `MediaEvent::Error` match sites, one `Playback` trait default and one override, **120–200 insertions**. It is re-deferred rather than cut, because **no clause can fail on it today**: a background project whose document is not playing cannot produce a playback incident, so it is correctness with no failing test behind it, which IN1 §9's own opening sentence forbids. **Nothing in Part B pre-builds for it. Owner: the slice that makes the engine per-project, or that tags `MediaEvent`.** IN1 §13 D14's owner line changes accordingly, and §1 non-goal 3 names it.
- **D-B6 — a per-subject asset or clip name on the incident.** With seven subject kinds, `subject_label` is now `"Clip 4"` and `"Bus 3"` as well as `"Asset 1"`, and an agent that wants to name the thing calls `get_timeline_state`. This is IN1 §13 D15 widened by six subjects. **Not free:** the router must read the document at observe time, which is the coupling IN1 §2.3c rule 31 exists to avoid. **Cost:** 80–150 insertions and one new document read on the observe path. **Owner: IN2.** **Owner line added at IN2 Part A's promotion (2026-09-16):** **Owner: IN2 Part B** (`IN2B-INCIDENT-PERSISTENCE.md`) — it matters more after Part A than before it, because a proposal card that says *“Clip 4”* rather than the clip's name is the card a person has to approve ([IN2](IN2-INVESTIGATOR-SESSIONS.md) §13). **Owner line added at IN2 Part B's promotion (2026-09-22):** **Discharged by IN2 Part B** ([IN2B-INCIDENT-PERSISTENCE.md](IN2B-INCIDENT-PERSISTENCE.md) §9) — `IncidentObservation.name` captured by the app router, media clips via their asset, titles via a bounded text prefix, Master as "Master".
- **D-B7 — `IncidentSeverity::Informs`.** The variant is declared and no Part B row uses it (§0.2/h), because nothing in the 132 rows is purely advisory. **Not free only in the sense that it is not free to *use*:** the first advisory code needs a surface that does not demand action, which the badge and the card do not have. **Cost:** one `POLICY` row and one card affordance, 30–60 insertions, whenever the first advisory code arrives. **Owner: IN3**, whose efficiency budgets and QC observations are the first that will.
- Unchanged and still owned elsewhere: **IN1 §13 D1, D2, D5, D6, D7, D8, D9, D10, D11, D12, D13, D15**. Part B touches none of them.

---

## 14. Files

| Implementer | Crate | Files, exhaustively |
| --- | --- | --- |
| **A** | core | `src/operation.rs` (`IncidentFamily`, the grouped `incident_family()` with its one `#[allow(clippy::too_many_lines)]`, `incident_code()`, `Operation::incident_subject()`, the four tests); `src/time.rs` (`TimeMappingError::incident_family()`); `src/incident.rs` (`IncidentCode` → 67, `POLICY` → 67 rows with severities, six `IncidentSubject` variants, the `IncidentEvidence` variants and `probed() -> Option`, `policy_class(&IncidentEvidence)`, subject-generic `policy_recovery`, `explain_body`, the four observation constructors, `from_media_error` made total, the rewritten tests); `src/actor.rs` (**the correlation token, first**: `CommandToken`, one field on `Command::DoIfRevision`, `Event::RevisionConflict` and `Event::DocumentChanged`, the actor's echo, the test pattern at `:697`, one test); `src/media.rs` (two `MediaError` variants, two `From` impls, two `recovery_code()` arms, **`AudioChain`'s `PartialOrd, Ord, Serialize` derives at `:1165`**); `src/captions.rs` — **read, not edited**; `src/lib.rs` (the `pub use incident::{…}`, `pub use operation::{IncidentFamily, …}` and `pub use actor::{CommandToken, …}` names). **No id newtype is added to `src/model.rs`** (N2.5/B2). `src/color.rs`, `src/delivery.rs`, `src/color_qc.rs` are **read, not edited** — their `code()`/`field()`/`recovery_action()` accessors are what core delegates to. |
| **B** | media | ~~**No file is edited.**~~ **Six are**, all by implementer A under row A's authority and the orchestrator's rulings: `src/lut.rs`, `src/lut_store.rs`, `src/room_tone_store.rs`, `src/cc4_fixtures.rs`, `src/compositor.rs`, `src/engine.rs`. `src/in1_fixtures.rs:67` is **read** and its assertions stay green (§9 regression R-B). Probe-2b T3 confirmed no `kinewright-media` file breaks on the token. The R20–R39 erratum range stays unused, but only because A made the changes. **Erratum (A-R5, 2026-09-16):** §3.9 rule 36's typed matte variants change what the two `From` impls produce, so six assertions that destructured `MediaError::Backend(message)` for a matte refusal had to move — `src/compositor.rs:6194`, `:6205`, `:6215` and `src/engine.rs:2699`, `:2768`, `:2800`, plus `crates/kinewright-core/tests/cc5_core_proof.rs:552` and `:581`. All are test-only and the rendered text is unchanged, so probe-2b T3's "implementer B still edits no file" holds for the **token** and not for §3.9. **Erratum (A-R12, 2026-09-16):** N4/CR-D1's `MediaError::Store` rewrites the two stores' `From` impls (`src/lut_store.rs:155`, `src/room_tone_store.rs:208`) — **production** lines — and cost more than the ruling foresaw: three files, **two further production sites** that were bypassing the `From` impl (`lut_store.rs:580`'s `copy_one`, `room_tone_store.rs:790`'s `too_large`), ten rewritten test assertions, and **three** sites that had to stay on `Backend` because they belong to `LutParseError` and `ColorPipelineError`. **Erratum (A-R14, 2026-09-16):** CR-D2 adds a fourth production file, `src/lut.rs`, whose `From<LutParseError> for MediaError` was the one conversion left behind on a **reachable, served** path — `LutStore::import_lut_asset` parses the file it imports — and whose omission silently regressed the `import_lut_asset` tool's served `code` (D-R67). `ColorPipelineError` is the **deferred half** and is named rather than left silent: it has no code accessor, no `From` impl onto `MediaError` and no path to either served result, so giving it codes is a slice of its own. |
| **C** | app | `src/error_ui.rs` (`push` module-private, `record_error` **deleted in the final commit**, `note_incident` added); `src/app.rs` (the 27 `record_error` sites, the three `error_log.push` bypasses, **`app.rs:1822`'s restore sink**, `load_error` → `Option<IncidentObservation>`, `pending_observations` `pub(crate)`, `RouterConflict` gaining `token` and losing `expected`, `RouterApply`'s token, the rewritten `reconcile_router_sends`, the drain arms at `:1578-1591` keeping `op`/`operations`, the test drain mirror at `:4286`, the badge at `:1875`/`:1879`, `note_incident`'s one call site in `audit_new_incident`, the rewritten tests); **`src/recovery.rs`** (`restore_status`'s new shape and the `Event::RevisionConflict` pattern at **`:1011`**); `src/incident_ui.rs` (the 70-row headline table, `revert_available`, the generalised `card_action_outcome`, the split detail row, the Incidents panel); `src/media_workflow.rs` (26 sites, `SourceEditRejection`/`RelinkRejection`/`RelinkRevisionConflict` codes, the two `DoIfRevision` senders at `:1096` and `:1310`); `src/chat_ui.rs` (13 sites **plus the two `BranchApplyOutcome::Conflict` arms at `:631-637` and `:726-728`**); `src/export_ui.rs` (13); `src/timeline_ui.rs` (9 + `ROOM_TONE_ERROR_CATEGORY`); `src/keys.rs` (8); `src/transcript_edit.rs` (7); `src/preview_ui.rs` (6); `src/media_bin.rs` (5 + the deliberate `:438` filter); `src/recording.rs` (5); `src/inspector_ui.rs` (3 + `error_category()` → `incident_code()`); `src/captions.rs` (2 + the re-typed wrapper); `src/look_browser_ui.rs` (1); `src/transport.rs` (1); `src/project.rs` (`ProjectSaveError::incident_code()`). |
| **D** | agent | `src/server.rs` (`verify_claimed_outcome` per code, `room_tone_store_error_result` and `lut_store_error_result` de-parsed, the two size constants, the ceiling loop over 67 × 7, the `Command::DoIfRevision` construction at `:1582` and the three `Event::RevisionConflict` patterns at `:1602`, `:1697`, `:2509`, the pins at `:22291`, `:26256-26298`); `src/export_queue.rs` (the third parser at `:412`); `src/branch.rs` (`BranchError::incident_code()` and the `RevisionConflict` pattern at `:193`); `tests/mcp_server.rs` (the pins at `:2427`, `:2561-2564`, `:10346`, `:10400-10401`, and the new `in1b_` tests). `src/schema.rs`, `src/runtime.rs`, `src/eval.rs` are **read, not edited**. |
| orchestrator | docs | `docs/IN1B-ERROR-MIGRATION.md` (this file, promoted); `docs/IN1-INCIDENTS-AND-THE-COLOUR-CASE.md` (§10 gains one pointer paragraph; §13 D3, D4, D14 and D16 gain owner lines; §0 gains a `0.5 Changes made by IN1b` subsection carrying the errata below); `docs/ROADMAP-AND-WORKFLOWS.md` (the IN1 status paragraph and the row's Part B clause; the sentence becomes **"IN1 lands in two parts on two contracts, IN1b discharging IN1 §10"**); `docs/M36-AGENT-RUNTIME-EFFICIENCY.md` (one row: the served quad unchanged after IN1b, seventeenth consecutive measurement); `CHANGELOG.md` (one entry). |

**Two implementer notes probe-2b measured and the house gate turns into build failures** (N2.5/probe 12). Both new `impl` blocks in `operation.rs` and `time.rs` **must** sit **before** their file's `#[cfg(test)] mod tests`, or `clippy::items_after_test_module` fires. Every bare `IN1b` in a doc comment **must** be backticked, or `clippy::doc_markdown` fires. Neither is large; both fail `cargo clippy --workspace --all-targets -- -D warnings`.

**Errata IN1b writes into the IN1 contract at promotion**, each by id. **These ids live in the `IN1b-` namespace** — `IN1b-R1a` … `IN1b-R16` — and do **not** share IN1 §14's per-crate implementation ranges (A R1–R19, B R20–R39, C R40–R59, D R60–R79), which this contract restarts for its own implementers (N0/Q1). Without the namespace there would be two R1s in one programme (the critic's S14). Each row quotes the parent text it amends; line numbers are `docs/IN1-INCIDENTS-AND-THE-COLOUR-CASE.md` at `d4ed8eb`.

| id | IN1 rule | the parent's words | change |
| --- | --- | --- | --- |
| R1a | §9 clause 1 (`:1103`), §2.3b rule 26 (`:346`) | *"`POLICY` is exhaustive over `IncidentCode` … **every row `Blocks`**"*; *"every Part A row is `IncidentSeverity::Blocks`, because a source-colour refusal stops the managed decode"* | becomes "every **colour** row blocks and every row's severity is the one it declares" (§3.6 rule 29). §2.3b rule 26's own reason is unchanged and is why the colour half survives |
| R1b | §9 clause 1 (`:1103`) | *"`POLICY.len() == 12` … equal to `ColorSourceError::code()`/`field()` for all **twelve**, with `UnknownWhitePoint` mapping to `None`"* | `POLICY.len() == 67`; thirteen-for-thirteen with **no `None` case** (§3.2 rules 12 and 16). This is the half §0.2/d promised and revision 1 did not deliver |
| R2 | §2.3c rule 28 (`:351`) | *"An error with no `IncidentCode` is not an incident; it stays a `record_error` line (§1 item 6)."* | superseded: after IN1b there is no `record_error` line to stay as (§5.3 rule 23) |
| R3 | §6.6 rule 25 (`:974`), §9 clause 13 (`:1124`) | *"Asserted unchanged in **both** pin sites … This is the **sixteenth** consecutive measurement"*; *"The served quad is `7 / 5 660 / 3 510 / 998` in **both** pin sites"* | "all **three** pin sites", **seventeenth** consecutive measurement (§6.4 rules 7–8) |
| R4 | §2.2 rule 6 (`:191`), rule 8 (`:209`) | *"one-to-one except `unknown_source_white_point` … **twelve incident codes from thirteen classifier variants**"*; *"every variant except `UnknownWhitePoint` maps to exactly one `SourceColorIncident` … and `UnknownWhitePoint` maps to `None`"* | thirteen from thirteen (§3.2 rule 16). Rule 6's *reason* — that the **bare** classifier reports it for every correctly tagged source — is unchanged and untouched; §3.2 rule 14 names the post-assumption sRGB-primaries path that makes the code reachable |
| R5 | §10 rule 6 (`:1176`) | *"`ErrorLog::push` becomes private to `error_ui.rs`, **the only writer is the incident router**"* | superseded by §5.3: the single writer is `KinewrightApp::note_incident`, a method on the app rather than the router, because `status` and `error_log_open` are app fields (N0/Q5, N1/B5) |
| R6 | §10 rule 1 (`:1148`) | *"**121** `record_error(` call sites with a literal source label over **15** labels, plus 2 dynamic-source sites, plus 3 direct `ErrorLog::push` bypasses — 123 call sites and **126** error paths in all. Part A migrates exactly 1 … Part B owns the remaining 120 + 2 + 3 = **125**."* | becomes 124 literal sites over **16** literal spellings (**17** reachable labels, N1.5 §4), 2 dynamic and 3 bypasses — **129** error paths at `d4ed8eb`, three of them added by Part A. The comparable pair is 126 → 129, not 125 → 129 (the critic's S13) |
| R7 | §10 rule 4 (`:1174`) | *"a `const fn OpError::incident_family(&self) -> IncidentFamily` with approximately **8** families — `Validation`, `Missing`, `RangeOrBounds`, `IdExhausted`, `Duplicate`, `Ordering`, `ColorPolicy`, `TimeOverflow` — … and decides per-variant codes from Part A's experience"* | **11** families, named in §3.1 rule 1; per-variant codes are **two**, not none (§3.1 rule 6) |
| R8 | §1 item 10 (`:152`), §13 D4 (`:1251`) | *"The toolbar badge still counts `self.error_log.len()` (`crates/kinewright-app/src/app.rs:1487-1491`). Part B settles its semantics."*; *"**Owner: IN1 Part B.**"* | discharged: the badge counts open incidents (§5.4 rule 25). The citation is also stale — at `d4ed8eb` the badge is `app.rs:1875` (`if self.error_log.len() > 0`) and `:1879` (`format!("{}", self.error_log.len())`) — and is re-cited while the erratum is there (the critic's N3) |
| R9 | §2.3b rule 22's implementation erratum (`:342`) | *"**Erratum (implementation, 2026-09-15):** `Some` for `SourceColorForAsset` **except when the carried refusal is `UnknownWhitePoint`**, which §2.2 rule 6 makes a non-incident and therefore also returns `None`; tested."* | **reversed** by §0.2/d: `from_media_error` returns `Some` for all thirteen. The same rule's *"the function returns `Some` for `SourceColorForAsset` and `None` for all ten others"* also becomes false: after §3.9 every `MediaError` variant has a code and the function is **total**, returning `IncidentObservation` rather than `Option` (§5.1 rule 12) |
| R10 | §9 clause 14 (`:1125`) | *"`<= IN1_INCIDENT_SERIALIZED_CEILING_BYTES` for all **twelve** codes on a **synthetic subject**"* | **67** codes across all **seven** subject shapes — 469 pairs (§9 clause 16). The ceiling constant itself moves from 1 024 to **2 048** and `IN1_INCIDENT_SERIALIZED_BYTES` stays **819** |
| R11 | §1 item 6 (`:148`) | *"The other **120** literal-label sites, the **2** dynamic-source sites … and the **3** `ErrorLog::push` bypasses … are untouched and stay `record_error`/`push` lines — **125** error paths for Part B. `ErrorLog::push` **stays `pub(crate)`**."* | superseded twice over: the population is 129 (R6), and `ErrorLog::push` becomes module-private (§5.3 rule 17) |
| R12 | §2.2 rule 9 (`:210`) | *"A failure with no code is not an incident in IN1 and stays a `record_error` line (§1 item 6)."* | the twin of R2, in a second place: after IN1b every measured failure has a code and there is no `record_error` line. The rest of rule 9 — no catch-all, no `Unclassified`, no `Other(String)` — is unchanged and §3.2 rule 10 keeps it |
| R13 | §1 item 15 (`:157`) | *"`Incident.subject` is `{\"asset\": n}`; the asset's `name` is a named deferral (§13 D15)."* | falsified by §3.11 rule 42's three-shape union: a one-key object over a `u64`, a one-key object over a nested union (`{"chain":{"bus":3}}`), or a bare string (`"project"`). The `name` half is unchanged and is now §13 D-B6 |
| R14 | §2.3b rule 25 (`:345`) | *"a core test asserts `incident.field == incident.code.field()` over all **twelve** codes anyway"* | over all **67** (§3.2 rule 11, §7 item 6). The rule's own property — `field` and `class` assigned at exactly one site each — is unchanged (§3.5 rule 27) |
| R15 | §2.4 rule 35 (`:373`) | *"asserting that each variant appears in `POLICY` exactly once and that **`POLICY.len() == 12`**"* | 67. This is the second copy of R1b's assertion and takes its own erratum so neither is missed |
| R16 | §10 rule 8 (`:1178`) | *"`IncidentSubject` grows to `Clip`, `Track`, `ExportJob`, `Project`, `Agent`."* | six variants, not five: `Chain(AudioChain)` joins them (N2.5/B3), and `ExportJob`, `Project` and `Agent` are **unit** variants carrying no id (N2.5/B2, §3.3 rule 18) |

**Order.** **A → (C ‖ D)**, with **the correlation token first inside A** — it is the only core change the app's router work depends on structurally, and landing it first lets C's rewrites and D's agent work proceed against a stable `actor.rs`. B edits nothing. C and D both depend on A and neither may assert a pinned byte count before §3.11 rule 43's figures are in place. C's own internal order is §5.1 rule 11's eight steps, with `record_error` surviving unchanged through steps 1–7 and deleted in step 8 (§5.3 rule 21). The workspace gate and `cargo check -p kinewright-app` run before **each** commit, and two review passes per crate precede each. Every cargo command runs after `source ./scripts/setup-ffmpeg.sh`.

IN1b is complete only when the application has no way to tell a person something went wrong on its log, its status bar or its chat transcript except by naming what went wrong, saying whether it stopped their work or merely reduced it, and saying in one sentence what they must change — and when the agent reading the same failure gets the same four things from `get_incidents` in one call, for the seventeenth consecutive measurement of zero additional served bytes.

---

## Appendix A — all 154 `OpError` variants in a family, normative

Declaration order at `crates/kinewright-core/src/operation.rs:494-1060` (`d4ed8eb`). Counts sum to **154** and the partition is exact: probe-1b P1 compiled it with `Self::Variant { .. }` arms for tuple, struct and unit variants alike, and probe-2b T6 re-parsed it independently against the enum; both found 0 duplicates, 0 omissions and 0 extras, and every declared family count equal to its own list length. N1.5 §9's seven corrections are applied and marked **†**; `incident_family()` groups these lists into 11 `|`-joined arms (§3.1 rule 3). **No finding of the contract critic or probe-2b names a variant in this table, so it is unchanged from revision 1.**

| family | n | variants |
| --- | ---: | --- |
| `Bounds` | 38 | `AudioBusLookaheadExceeded`, `AudioBusKeyframeOutsideProject`, `AudioBusGainOutOfRange`, `AudioMasterGainOutOfRange`, `AudioMasterKeyframeOutsideProject`, `AudioMasterLookaheadExceeded`, `SourceOutOfBounds`, `NegativeTimelinePosition`, `SplitOutsideClip`, `NegativeMarkerPosition`, `InvalidMarkerColor`, `InvalidRippleDuration`, `InvalidTitleDuration`, `InvalidFreezeDuration`, `FreezeSourceFrameOutOfRange`, `TitleParamOutOfRange`, `TitleTextTooLong`, `TitleFadeTooLong`, `EffectParamOutOfRange`, `TooManyColorNodes`, `TooManyLutNodes`, `EffectIndexOutOfRange`, `EffectKeyframeOutsideClip`, `InvalidTransitionDuration`, `TransitionTooLong`, `AudioGainOutOfRange`, `NegativeAudioFade`, `AudioFadesTooLong`, `ClipGainEnvelopeOutOfRange`, `ClipGainEnvelopeKeyframeOutsideClip`, `TrackAutomationOutOfRange`, `TrackAutomationKeyframeOutsideProject`, `AudioBusGainKeyframeOutsideProject`, `AudioMasterGainKeyframeOutsideProject`, `TrackMixGainOutOfRange`, `TrackMixPanOutOfRange`, `ClipSpeedOutOfRange`, **`ColorConfidenceOutOfRange`†** |
| `Malformed` | 25 | `EmptyBinName`, `InvalidStringOut`, `InvalidSyncGroup`, `EmptySyncAngle`, `InvalidAudioBus`, `InvalidSourceRange`, `InvalidThreePointSelection`, `EmptySourcePatch`, `InvalidThreePointSource`, `InvalidThreePointTimeline`, `InvalidProjectRate`, `InvalidAssetRate`, `InvalidAssetDuration`, `InvalidResolution`, `InvalidTitleParamType`, `InvalidMarkerParamType`, `InvalidEffectParamType`, `MissingCubeLutPath`, `InvalidLutAssetHash`‡, `InvalidLutAssetMetadata`‡, `InvalidEffectAutomation`, `InvalidClipGainEnvelope`, `InvalidTrackAutomation`, **`TooFewLinkedClips`†**, **`NonHoldKeyframeParameter`†** |
| `Duplicate` | 20 | `DuplicateAsset`, `DuplicateBin`, `DuplicateBinAsset`, `AssetInMultipleBins`, `DuplicateStringOut`, `DuplicateSyncGroup`, `DuplicateSyncGroupAsset`, `DuplicateAudioBus`, `TrackInMultipleAudioBuses`, `DuplicateAudioBusEffect`, `DuplicateAudioMasterEffect`, `DuplicateTrack`, `DuplicateClip`, `DuplicateSourcePatchTrack`, `DuplicateClipSelection`, `DuplicateMarker`, `DuplicateEffect`, `DuplicateLutAsset`, `DuplicateTransition`, `DuplicateTrackMix` |
| `Structure` | 17 | `BinSelfParent`, `BinCycle`, `BinHasChildren`, `ClipOverlap`, `ClipsUnsorted`, `ClipsNotAdjacent`, `SlideRequiresNeighbors`, `MarkersUnsorted`, `CaptionPresetMismatch`, `CurvePointCountAnimatedWithPoints`, `LutAssetInUse`, `ColorStageOrderViolation`, `TrackMixUnsorted`, **`InvalidCurvePoints`†**, **`NewTrackNotEmpty`†**, **`AudioBusDuckingWithoutSidechain`†**, **`AudioMasterDuckingUnsupported`†** |
| `Placement` | 13 | `VisualEffectOnAudioBus`, `AudioEffectOnClip`, `VisualEffectOnAudioMaster`, `IncompatibleTrack`, `TitleOnAudioTrack`, `FreezeOnAudioTrack`, `EditorialRequiresMedia`, `InvalidSourcePatchRouteKind`, `NotTitleClip`, `NotALegacyLook`, `TitleClipHasNoAudio`, `FreezeClipHasNoAudio`, `SpeedOnNonMediaClip` |
| `Missing` | 13 | `MissingBin`, `MissingStringOut`, `MissingSyncGroup`, `MissingAudioBus`, `AudioBusMissingTrack`, `MissingAsset`, `MissingTrack`, `MissingClip`, `MissingMarker`, `MissingEffect`, `MissingLutAsset`, `UnknownLutAsset`, `MissingTransition` |
| `Relink` | 8 | `SourceFingerprintIncomplete`, `InvalidSourceFingerprintHash`, `InvalidSourceFingerprintByteLength`, `EmptyRelinkCandidatePath`, `UnverifiedRelinkCandidate`, `RelinkMetadataMismatch`, `RelinkFingerprintMismatch`, `RelinkRequiresExplicitUnverifiedSource` |
| `Unrepresentable` | 6 | `ZeroProjectDuration`, `UnrepresentableSplit`, `UnrepresentableEditBoundary`, `ReplacementDurationMismatch`, `FitToFillUnrepresentable`, `IncorrectDocumentDuration` |
| `UnknownName` | 6 | `UnknownTitleParam`, `UnknownMarkerParam`, `UnknownEffect`, `UnknownEffectParam`, `UnknownTransition`, `UnknownTrackAutomationParameter` |
| `Internal` | 5 | `ClipIdExhausted`, `LinkIdExhausted`, `LutAssetIdExhausted`, `TimeOverflow`, **`TimeMapping`†** — a delegating arm; its family is `TimeMappingError::incident_family()`'s (§3.1 rule 4), so only `TimeMappingError::Overflow` is actually `Internal` |
| `ColorPolicy` | 3 | `ZeroConfidenceColorOverride`, `InvalidColorOverrideProvenance`, `AssumedFromNotSuppliable` — kept **as a routing hint to IN1's own card**, not as a claim of a distinct recovery (§3.1 rule 7) |

**†** moved by N1.5 §9 from the design brief's Appendix A. **‡** stays in `Malformed` for the family map and takes the minted per-variant code `lut_asset_policy` (§3.1 rule 6). `IncorrectDocumentDuration` stays in `Unrepresentable` against the brief critic's S5: the recovery is to recompute the duration, and no value the caller supplies would help.

---

## Appendix B — all 132 error paths, with their target

**132 rows: the 129 measured `record_error`/`error_log.push` paths of §2.1, plus the three log-bypassing sinks of §5.7 (rows 28, 39 and 43), which carry no source label and are in no per-label count.** `file:line` at `d4ed8eb` under `crates/kinewright-app/src/`. `evidence` names the `IncidentEvidence` variant of §3.4 rule 23. A row reading `operation_*` takes its code from `OpError::incident_code()` at run time (§3.1 rule 6); a row reading `MediaError::recovery_code()` takes it from that accessor (§3.2 rules 11 and 14). A subject reading *(from the operation)* or *(from the batch)* is `Operation::incident_subject()`'s (§3.3 rule 20). Severity is derived per §3.6 rule 30 from the site's own control flow and is declared once per code.

**Erratum (C-R41, 2026-09-16):** seven rows this appendix declares `Clip` take **`Project`**, amended in their subject cells below, because no `ClipId` is in scope at any of them and §3.3 rule 22 rejects inventing one: rows 117 and 121 (`timeline_ui.rs:632`, `:713`) are id-space exhaustion, which is document-wide; row 118 plans a freeze from the playhead and names no clip; rows 119 and 120 fire **because** there is no clip, which is this appendix's own correction for row 65; row 128 is a transcript scan that failed before any cut range existed and row 129 fires because nothing was selected. Rows 126, 127, 130 and 131 keep `Clip`, because two small helpers resolve the plan's own first cut (`transcript_edit.rs:162`, `:170`).

**Erratum (C-R52, 2026-09-16), narrowing the withdrawn C-R42:** the seven `Look` rows this appendix declares `Asset` are amended below to **`LutAsset`** (rows 96, 101, 102, 103, 104) now that erratum A-R13 declares the variant, and to **`Project`** for rows 99 and 100, which fire *before* a `LutAssetId` exists — row 99's import is the thing that failed and row 100's `lut_import_operations` is the plan that mints the id — so there is no look to state them against and the store root they name is the project's. `LutRestoreResponse` gains `lut_asset: LutAssetId` (`crates/kinewright-app/src/media_workflow.rs:1753`) so row 104 names the look rather than the project.

**Erratum (C-R48, 2026-09-16):** rows 99 and 104 keep `MediaError` evidence, but its `code` field is the **incident's own** (`look_unclassified`) and not the store's, because erratum A-R1's amendment makes "one incident carries one code" structural in core's `media_evidence`; the engine's rendered reason survives in `observed` (`crates/kinewright-app/src/media_workflow.rs:2017`).

**Erratum (C-R51, 2026-09-16):** row 26's observation is **skipped** when the conflict's token is one the router holds, decided once in `KinewrightApp::note_core_event_for_router` (`crates/kinewright-app/src/app.rs:1314`). Raising an `edit_revision_conflict` card for a send Kinewright planned, sent and is already reconciling gives the person a permanently-open card telling them to re-make an edit they never made; the skip uses §4 rule 7's identity and nothing else, and every foreign conflict still gets its card.

**Erratum (C-R54, 2026-09-16), narrowing C-R51 and C-R53:** the two skips apply only to a send the router planned — `RouterApply` carries `origin: RouterSendOrigin { Auto, CardPress }` (`crates/kinewright-app/src/app.rs:93`) — so a **card press** that loses its race still gets its answer: the apply is dropped, and rows 26 and 24's observation is raised against **the incident's own subject** rather than being suppressed. Without it a person pressed a button and nothing happened at all; §4 rule 7 carries the full erratum.

| # | site (`crates/kinewright-app/src/…`) | label | `IncidentCode` | subject | evidence | class | severity |
| ---: | --- | --- | --- | --- | --- | --- | --- |
| 1 | `app.rs:470` | (dynamic) | `project_unclassified` | `Project` | `Plain` | `Explain` | `Blocks` |
| 2 | `app.rs:487` | Media | `media_incomplete` | `Project` | `Plain` | `Explain` | `Degrades` |
| 3 | `app.rs:627` | Look | `look_incomplete` | `Project` | `Plain` | `Explain` | `Degrades` |
| 4 | `app.rs:660` | Look | `look_incomplete` | `Project` | `Plain` | `Explain` | `Degrades` |
| 5 | `app.rs:671` | Project | `project_save_failed` | `Project` | `ProjectSave` | `Explain` | `Blocks` |
| 6 | `app.rs:695` | Project | `project_unclassified` | `Project` | `Plain` | `Explain` | `Blocks` |
| 7 | `app.rs:711` | Project | `project_unclassified` | `Project` | `Plain` | `Explain` | `Blocks` |
| 8 | `app.rs:730` | Project | `project_unclassified` | `Project` | `Plain` | `Explain` | `Blocks` |
| 9 | `app.rs:751` | Look | `look_incomplete` | `Project` | `Plain` | `Explain` | `Degrades` |
| 10 | `app.rs:760` | Look (bypass) | `look_incomplete` | `Project` | `Plain` | `Explain` | `Degrades` |
| 11 | `app.rs:775` | Media (bypass) | `media_incomplete` | `Project` | `Plain` | `Explain` | `Degrades` |
| 12 | `app.rs:951` | Operations | `operation_internal` | `Project` | `Plain` | `Explain` | `Blocks` |
| 13 | `app.rs:1074` | Incident | *not migrated* | — | — | — | — |
| 14 | `app.rs:1088` | Operations | `operation_internal` | `Project` | `Plain` | `Explain` | `Blocks` |
| 15 | `app.rs:1251` | Operations | `operation_internal` | `Project` | `Plain` | `Explain` | `Blocks` |
| 16 | `app.rs:1273` | Operations | `operation_internal` | `Project` | `Plain` | `Explain` | `Blocks` |
| 17 | `app.rs:1316` | Operations | `operation_internal` | `Project` | `Plain` | `Explain` | `Blocks` |
| 18 | `app.rs:1334` | Operations | `operation_internal` | `Project` | `Plain` | `Explain` | `Blocks` |
| 19 | `app.rs:1342` | Operations | `operation_internal` | `Project` | `Plain` | `Explain` | `Blocks` |
| 20 | `app.rs:1377` | Media (bypass) | `media_incomplete` | `Asset` | `Plain` | `Explain` | `Degrades` |
| 21 | `app.rs:1407` | Operations | `operation_internal` | `Project` | `Plain` | `Explain` | `Blocks` |
| 22 | `app.rs:1410` | Media | `media_unclassified` | `Project` | `Plain` | `Explain` | `Blocks` |
| 23 | `app.rs:1472` | Source monitor | `source_monitor_unclassified` | `Asset` | `Plain` | `Explain` | `Blocks` |
| 24 | `app.rs:1580` | Operations | `operation_* (family)` | *(from the operation)*; **the incident's own** for a refused card press — errata **C-R53**, **C-R54** | `OpError` | `Explain` | `Blocks` — **skipped only for the router's `Auto` sends** |
| 25 | `app.rs:1585` | Operations | `edit_plan_rejected \| operation_*` | *(from the batch)* | `OpError` | `Explain` | `Blocks` |
| 26 | `app.rs:1598` | Operations | `edit_revision_conflict` | `Project`; **the incident's own** for a refused card press — erratum **C-R54** | `Revision` | `Explain` | `Blocks` — **skipped only for the router's `Auto` sends**, errata **C-R51**, **C-R54** |
| 27 | `app.rs:1637` | Media | `MediaError::recovery_code()` (§3.2) | `Asset` (the 13 `SourceColor` codes) \| `Project` (`unsupported_decoder_format`, `media_backend_unclassified`) — erratum **A-R2** | `SourceColor \| MediaError` | `AutoApply \| Explain` | `Blocks` |
| 28 | `app.rs:1822` + `recovery.rs:899-904` | *(none — status bar)* | `project_unclassified` | `Project` | `Plain` | `Explain` | `Blocks` |
| 29 | `app.rs:2814` | Mixer | `mixer_unclassified` | `Chain` | `Plain` | `Explain` | `Blocks` |
| 30 | `app.rs:2850` | Mixer | `mixer_unclassified` | `Chain` | `Plain` | `Explain` | `Blocks` |
| 31 | `app.rs:2876` | Mixer | `mixer_unclassified` | `Chain` | `Plain` | `Explain` | `Blocks` |
| 32 | `captions.rs:55` | Captions | `captions_unclassified` | `Project` | `Plain` | `Explain` | `Blocks` |
| 33 | `captions.rs:61` | Captions | `caption_plan_rejected \| operation_internal` | `Project` | `CaptionPlan` | `Explain` | `Blocks` |
| 34 | `chat_ui.rs:391` | Agent | `agent_unclassified` | `Agent` | `Plain` | `Explain` | `Blocks` |
| 35 | `chat_ui.rs:401` | Agent | `agent_unclassified` | `Agent` | `Plain` | `Explain` | `Blocks` |
| 36 | `chat_ui.rs:443` | Agent | `agent_unclassified` | `Agent` | `Plain` | `Explain` | `Blocks` |
| 37 | `chat_ui.rs:477` | Agent | `agent_unclassified` | `Agent` | `Plain` | `Explain` | `Blocks` |
| 38 | `chat_ui.rs:549` | Agent branch | `agent_branch_unclassified` | `Agent` | `Plain` | `Explain` | `Blocks` |
| 39 | `chat_ui.rs:631-637` | *(none — chat + status)* | `edit_revision_conflict` | `Agent` | `Revision` | `Explain` | `Blocks` |
| 40 | `chat_ui.rs:640` | Agent branch | `edit_plan_rejected \| operation_*` | *(from the batch)* | `OpError` | `Explain` | `Blocks` |
| 41 | `chat_ui.rs:642` | Agent branch | `agent_branch_rejected \| operation_*` | `Agent` \| *(from the operation)* | `Branch \| OpError` | `Explain` | `Blocks` |
| 42 | `chat_ui.rs:654` | Agent branch | `agent_branch_rejected` | `Agent` | `Branch` | `Explain` | `Blocks` |
| 43 | `chat_ui.rs:726-728` | *(none — status bar)* | `edit_revision_conflict` | `Agent` | `Revision` | `Explain` | `Blocks` |
| 44 | `chat_ui.rs:727` | Agent branch | `edit_plan_rejected \| operation_*` | *(from the batch)* | `OpError` | `Explain` | `Blocks` |
| 45 | `chat_ui.rs:729` | Agent branch | `agent_branch_rejected \| operation_*` | `Agent` \| *(from the operation)* | `Branch \| OpError` | `Explain` | `Blocks` |
| 46 | `chat_ui.rs:797` | Branch preview | `MediaError::recovery_code()` (§3.2) | `Agent` | `MediaError` | `Explain` | `Blocks` |
| 47 | `chat_ui.rs:849` | Agent | `agent_unclassified` | `Agent` | `Plain` | `Explain` | `Blocks` |
| 48 | `chat_ui.rs:941` | Agent branch | `agent_branch_unclassified` | `Agent` | `Plain` | `Explain` | `Blocks` |
| 49 | `export_ui.rs:1543` | Export | `delivery_variant_rejected` | `ExportJob` | `DeliveryVariant` | `Explain` | `Blocks` |
| 50 | `export_ui.rs:1550` | Export | `delivery_variant_rejected` | `ExportJob` | `DeliveryVariant` | `Explain` | `Blocks` |
| 51 | `export_ui.rs:1566` | Export | `delivery_variant_rejected` | `ExportJob` | `DeliveryVariant` | `Explain` | `Blocks` |
| 52 | `export_ui.rs:1571` | Export | `export_unclassified` | `ExportJob` | `Plain` | `Explain` | `Blocks` |
| 53 | `export_ui.rs:1576` | Export | `export_unclassified` | `ExportJob` | `Plain` | `Explain` | `Blocks` |
| 54 | `export_ui.rs:1585` | Export | `export_unclassified` | `ExportJob` | `Plain` | `Explain` | `Blocks` |
| 55 | `export_ui.rs:1643` | Export | `export_unclassified` | `ExportJob` | `Plain` | `Explain` | `Blocks` |
| 56 | `export_ui.rs:1649` | Export | `export_unclassified` | `ExportJob` | `Plain` | `Explain` | `Blocks` |
| 57 | `export_ui.rs:1658` | Export | `export_unclassified` | `ExportJob` | `Plain` | `Explain` | `Blocks` |
| 58 | `export_ui.rs:1664` | Export | `export_unclassified` | `ExportJob` | `Plain` | `Explain` | `Blocks` |
| 59 | `export_ui.rs:1744` | Export | `export_unclassified` | `ExportJob` | `Plain` | `Explain` | `Blocks` |
| 60 | `export_ui.rs:1781` | Captions | `captions_unclassified` | `Project` | `Plain` | `Explain` | `Blocks` |
| 61 | `export_ui.rs:1852` | Export | `MediaError::recovery_code()` (§3.2) | `ExportJob` | `MediaError` | `Explain` | `Blocks` |
| 62 | `inspector_ui.rs:793` | (dynamic) | `look_unclassified \| timeline_unclassified` | `Clip` | `Plain` | `Explain` | `Blocks` |
| 63 | `inspector_ui.rs:862` | Look | `operation_internal` | `Clip` | `Plain` | `Explain` | `Blocks` |
| 64 | `inspector_ui.rs:1022` | Operations | `operations_unclassified` | `Clip` | `Plain` | `Explain` | `Blocks` |
| 65 | `keys.rs:344` | Operations | `operations_unclassified` | `Project` | `Plain` | `Explain` | `Blocks` |
| 66 | `keys.rs:351` | Operations | `operations_unclassified` | `Clip` | `Plain` | `Explain` | `Blocks` |
| 67 | `keys.rs:355` | Operations | `operations_unclassified` | `Clip` | `Plain` | `Explain` | `Blocks` |
| 68 | `keys.rs:366` | Operations | `operations_unclassified` | `Clip` | `Plain` | `Explain` | `Blocks` |
| 69 | `keys.rs:372` | Operations | `operations_unclassified` | `Clip` | `Plain` | `Explain` | `Blocks` |
| 70 | `keys.rs:385` | Operations | `operations_unclassified` | `Clip` | `Plain` | `Explain` | `Blocks` |
| 71 | `keys.rs:397` | Operations | `operations_unclassified` | `Clip` | `Plain` | `Explain` | `Blocks` |
| 72 | `keys.rs:406` | Operations | `operations_unclassified` | `Clip` | `Plain` | `Explain` | `Blocks` |
| 73 | `look_browser_ui.rs:303` | Look | `look_unclassified` | `Project` | `Plain` | `Explain` | `Blocks` |
| 74 | `media_bin.rs:86` | Operations | `operations_unclassified` | `Asset` | `Plain` | `Explain` | `Blocks` |
| 75 | `media_bin.rs:104` | Operations | `operations_unclassified` | `Asset` | `Plain` | `Explain` | `Blocks` |
| 76 | `media_bin.rs:121` | Operations | `operation_internal` | `Asset` | `Plain` | `Explain` | `Blocks` |
| 77 | `media_bin.rs:139` | Operations | `operation_internal` | `Asset` | `Plain` | `Explain` | `Blocks` |
| 78 | `media_bin.rs:146` | Operations | `operations_unclassified` | `Asset` | `Plain` | `Explain` | `Blocks` |
| 79 | `media_workflow.rs:856` | Media | `media_unclassified` | `Asset` | `Plain` | `Explain` | `Blocks` |
| 80 | `media_workflow.rs:994` | Source monitor | `source_monitor_unclassified` | `Asset` | `Plain` | `Explain` | `Blocks` |
| 81 | `media_workflow.rs:1069` | Source monitor | `source_monitor_unclassified` | `Asset` | `Plain` | `Explain` | `Blocks` |
| 82 | `media_workflow.rs:1084` | Source monitor | `source_edit_rejected` | `Asset` | `SourceEdit` | `Explain` | `Blocks` |
| 83 | `media_workflow.rs:1088` | Source monitor | `source_monitor_unclassified` | `Asset` | `Plain` | `Explain` | `Blocks` |
| 84 | `media_workflow.rs:1102` | Operations | `operation_internal` | `Asset` | `Plain` | `Explain` | `Blocks` |
| 85 | `media_workflow.rs:1199` | Relink | `edit_revision_conflict` | `Asset` | `Revision` | `Explain` | `Blocks` |
| 86 | `media_workflow.rs:1232` | Relink | `relink_rejected` | `Asset` | `Relink` | `Explain` | `Blocks` |
| 87 | `media_workflow.rs:1236` | Relink | `relink_unclassified` | `Asset` | `Plain` | `Explain` | `Blocks` |
| 88 | `media_workflow.rs:1277` | Media cache | `media_cache_unclassified` | `Project` | `Plain` | `Explain` | `Blocks` |
| 89 | `media_workflow.rs:1300` | Relink | `edit_revision_conflict` | `Asset` | `Revision` | `Explain` | `Blocks` |
| 90 | `media_workflow.rs:1316` | Relink | `operation_internal` | `Asset` | `Plain` | `Explain` | `Blocks` |
| 91 | `media_workflow.rs:1372` | Relink | `edit_revision_conflict` | `Asset` | `Revision` | `Explain` | `Blocks` |
| 92 | `media_workflow.rs:1380` | Relink | `relink_unclassified` | `Asset` | `Plain` | `Explain` | `Blocks` |
| 93 | `media_workflow.rs:1399` | Relink | `relink_rejected` | `Asset` | `Relink` | `Explain` | `Blocks` |
| 94 | `media_workflow.rs:1853` | Look | `look_unclassified` | `Project` | `Plain` | `Explain` | `Blocks` |
| 95 | `media_workflow.rs:1879` | Look | `look_unclassified` | `Project` | `Plain` | `Explain` | `Blocks` |
| 96 | `media_workflow.rs:1886` | Look | `look_unclassified` | `LutAsset` — erratum **C-R52** | `Plain` | `Explain` | `Blocks` |
| 97 | `media_workflow.rs:1902` | Look | `look_unclassified` | `Project` | `Plain` | `Explain` | `Blocks` |
| 98 | `media_workflow.rs:1929` | Look | `look_unclassified` | `Project` | `Plain` | `Explain` | `Blocks` |
| 99 | `media_workflow.rs:1941` | Look | `look_unclassified` \[cut 2 → `lut_store_*`\] | `Project` — erratum **C-R52** | `MediaError` (code = the incident's own, erratum **C-R48**) | `Explain` | `Blocks` |
| 100 | `media_workflow.rs:1953` | Look | `look_unclassified` | `Project` — erratum **C-R52** | `Plain` | `Explain` | `Blocks` |
| 101 | `media_workflow.rs:1966` | Look | `look_unclassified` | `LutAsset` — erratum **C-R52** | `Plain` | `Explain` | `Blocks` |
| 102 | `media_workflow.rs:1974` | Look | `operation_internal` | `LutAsset` — erratum **C-R52** | `Plain` | `Explain` | `Blocks` |
| 103 | `media_workflow.rs:1982` | Look | `look_incomplete` | `LutAsset` — erratum **C-R52** | `Plain` | `Explain` | `Degrades` |
| 104 | `media_workflow.rs:2006` | Look | `look_unclassified` \[cut 2 → `lut_store_*`\] | `LutAsset` — erratum **C-R52** | `MediaError` (code = the incident's own, erratum **C-R48**) | `Explain` | `Blocks` |
| 105 | `preview_ui.rs:1492` | Source monitor | `source_monitor_unclassified` | `Asset` | `Plain` | `Explain` | `Blocks` |
| 106 | `preview_ui.rs:1499` | Source monitor | `source_monitor_unclassified` | `Asset` | `Plain` | `Explain` | `Blocks` |
| 107 | `preview_ui.rs:1503` | Source monitor | `source_monitor_unclassified` | `Asset` | `Plain` | `Explain` | `Blocks` |
| 108 | `preview_ui.rs:1510` | Source monitor | `source_monitor_unclassified` | `Asset` | `Plain` | `Explain` | `Blocks` |
| 109 | `preview_ui.rs:1514` | Source monitor | `source_monitor_unclassified` | `Asset` | `Plain` | `Explain` | `Blocks` |
| 110 | `preview_ui.rs:1538` | Source monitor | `source_monitor_unclassified` | `Asset` | `Plain` | `Explain` | `Blocks` |
| 111 | `recording.rs:1002` | Recording | `recording_unclassified` | `Project` | `Plain` | `Explain` | `Blocks` |
| 112 | `recording.rs:1009` | Recording | `recording_unclassified` | `Project` | `Plain` | `Explain` | `Blocks` |
| 113 | `recording.rs:1021` | Recording | `recording_unclassified` | `Project` | `Plain` | `Explain` | `Blocks` |
| 114 | `recording.rs:1033` | Recording | `recording_unclassified` | `Project` | `Plain` | `Explain` | `Blocks` |
| 115 | `recording.rs:1060` | Recording | `recording_unclassified` | `Project` | `Plain` | `Explain` | `Blocks` |
| 116 | `timeline_ui.rs:581` | Operations | `operations_unclassified` | `Project` | `Plain` | `Explain` | `Blocks` |
| 117 | `timeline_ui.rs:632` | Operations | `operations_unclassified` | `Project` — erratum **C-R41** | `Plain` | `Explain` | `Blocks` |
| 118 | `timeline_ui.rs:652` | Operations | `operations_unclassified` | `Project` — erratum **C-R41** | `Plain` | `Explain` | `Blocks` |
| 119 | `timeline_ui.rs:665` | Operations | `operations_unclassified` | `Project` — erratum **C-R41** | `Plain` | `Explain` | `Blocks` |
| 120 | `timeline_ui.rs:701` | Operations | `operations_unclassified` | `Project` — erratum **C-R41** | `Plain` | `Explain` | `Blocks` |
| 121 | `timeline_ui.rs:713` | Operations | `operations_unclassified` | `Project` — erratum **C-R41** | `Plain` | `Explain` | `Blocks` |
| 122 | `timeline_ui.rs:734` | Operations | `operations_unclassified` | `Clip` | `Plain` | `Explain` | `Blocks` |
| 123 | `timeline_ui.rs:744` | Operations | `operations_unclassified` | `Clip` | `Plain` | `Explain` | `Blocks` |
| 124 | `timeline_ui.rs:3266` | Media | `media_unclassified` | `Track` | `Plain` | `Explain` | `Blocks` |
| 125 | `transcript_edit.rs:58` | Transcript edit | `transcript_edit_unclassified` | `Project` | `Plain` | `Explain` | `Blocks` |
| 126 | `transcript_edit.rs:64` | Transcript edit | `transcript_edit_unclassified` | `Clip` | `Plain` | `Explain` | `Blocks` |
| 127 | `transcript_edit.rs:70` | Transcript edit | `transcript_edit_unclassified` | `Clip` | `Plain` | `Explain` | `Blocks` |
| 128 | `transcript_edit.rs:95` | Transcript edit | `transcript_edit_unclassified` | `Project` — erratum **C-R41** | `Plain` | `Explain` | `Blocks` |
| 129 | `transcript_edit.rs:108` | Transcript edit | `transcript_edit_unclassified` | `Project` — erratum **C-R41** | `Plain` | `Explain` | `Blocks` |
| 130 | `transcript_edit.rs:115` | Transcript edit | `transcript_edit_unclassified` | `Clip` | `Plain` | `Explain` | `Blocks` |
| 131 | `transcript_edit.rs:121` | Transcript edit | `transcript_edit_unclassified` | `Clip` | `Plain` | `Explain` | `Blocks` |
| 132 | `transport.rs:14` | Media | `media_unclassified` | `Project` | `Plain` | `Explain` | `Blocks` |

**Row 13 (`app.rs:1074`) is Part A's own audit line and is not migrated**: it becomes the single `note_incident` call site (§5.3 rule 18). It is in the 129 by the counting rule (§2.1 rule 2) because the `record_error(` symbol it uses must be gone.

**Reading the table.** *Evidence follows the code* (§3.4 rule 24): where a row names two codes it names the evidence of each, and a delegating variant never falls back to a `reason` string. Six rows resolve a code at run time. Rows 24, 40 and 44 (`app.rs:1580`, `chat_ui.rs:640`, `:727`) resolve an `OpError` family through `incident_code()` and carry `OpError` evidence throughout; their subject is the rejected operation's. Row 25 (`app.rs:1585`) resolves a `BatchError`: `Empty` gives `edit_plan_rejected` with `OpError { family: Malformed, op_number: None }`, `OperationFailed` gives the inner code with the inner family and `op_number: Some(n)`. Rows 41 and 45 (`chat_ui.rs:642`, `:729`) resolve a `BranchError`: four variants give `agent_branch_rejected` with `Branch { reason }` and subject `Agent`, and `InvalidBase(OpError)` gives the inner code with `OpError` evidence and the operation's subject. Rows 27, 46 and 61 resolve a `MediaError` through `recovery_code()`, expanded code by code in §3.2 rule 14; row 27 is also the only path on which an `AutoApply` class is still reachable, and it is the **merged** site after §5.1 rule 12 makes `from_media_error` total. Row 62 (`inspector_ui.rs:793`) resolves one of two codes through `InspectorEdits::incident_code()` (§5.4 rule 27), and row 33 (`captions.rs:61`) resolves `caption_plan_rejected` or `operation_internal` through `CaptionPlanError::incident_code()` (§3.10 rule 39). The two `[cut 2 → lut_store_*]` rows take the typed `LutStoreErrorCode` if cut item 2 survives and `look_unclassified` if it does not (§12).

**What revision 2 corrected in this table.** Seventeen rows the contract critic marked wrong and fifteen it marked partial were re-read against the tree and changed, and three rows were added. **Fourteen rows take `operation_internal`** because their message begins "Core actor stopped" and their placeholder body was false (§3.2 rule 15): rows 12, 14, 15, 16, 17, 18, 19, 21, 63, 76, 77, 84, 90 and 102. **Row 9** (`app.rs:751`) becomes `look_incomplete`/`Degrades`: the session is pushed and focused at `app.rs:747-749` *before* the refusal, so the project opened, and row 3 (`app.rs:627`, the same store-root refusal on the save path) already said so. **Rows 29–31** (the three Mixer sites) take `Chain`: all three are `AudioChain`-scoped and no `TrackId` is in scope at `app.rs:2814`, `:2850` or `:2876`. **Rows 65, 116 and 125** take `Project`: `keys.rs:344` fires *because* no clip is selected, `timeline_ui.rs:581`'s arm is `EnvelopeDelete::RefuseLastKey` which carries no clip, and `transcript_edit.rs:58` runs over the whole timeline with `None`. **Row 60** (`export_ui.rs:1781`) takes `Project`: it is a `std::fs::write` of a caption sidecar and not an export job. **Rows 24, 25, 40, 41, 44 and 45** take a derived subject and a code-following evidence. **Row 124** keeps `Track`, and is the only row that constructs it: `RoomToneCaptureJob::track` is a `TrackId` (`timeline_ui.rs:3315`) destructured at `:3223` and still in scope at the refusal, which is why §3.3 rule 17 declares the variant at all.
