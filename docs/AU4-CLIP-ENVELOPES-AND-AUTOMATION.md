# AU4 Clip envelopes and automation

Status: promoted contract (revision 2, 2026-09-09); working files in target/review/au4/. §0 gains
implementation errata as the code lands, as AU2 §0 and AU3 §0 did. Built from the AU4 design brief
(D1–D14, §5's survival table, §10's evidence map, §11's deferrals) as amended by its critic report
(`critic-brief.md` C1–C22, its answers to Q1–Q8 and its "what the brief silently drops" list), by
`orchestrator-notes.md` N0, N0.1, N0.2 and **N1**, and **as amended by `critic-contract.md` F1–F29
and its rulings on revision 1's four open notes, under N2**, against main at de0beb3 (`fb2dcf3` in
the facts sheets). N1 and N2 accept every critic finding as written unless they amend it, and where
a ruling and an earlier document disagree the ruling wins. Every finding is folded into the rule,
table cell or checklist item it belongs to and registered in §0 as R25–R52; nothing is carried as an
appendix. The numbers in this document are the contract; implementation follows them or amends this
file first.

AU4 is the fourth slice of the audio programme (ROADMAP-AND-WORKFLOWS.md:648). It lands as **one
contract in two parts, two commits**: **Part A — Automation model and evaluation** (the five curve
owners, the two operations and their validation, the survival policy in every clip-moving
operation, the shared per-sample evaluation used by both mix paths, the live retarget path, the
agent operations and the `render_*` spellings) and **Part B — Automation editing and planners**
(the timeline rubber band, the Mixer `AUTOMATION` section and automated faders, the inspector
keyframe list, `plan_audio_ducking`, `plan_clip_fades`, DESIGN.md). Part A lands first and is a
complete exit gate on its own. Each part runs the full workspace gate and both review passes.

## 0. Errata

The implementation records each deliberate deviation from the text below here, as AU2 §0 and AU3 §0
do. Two figures are regenerated as the code lands: Part A's registry quad after the two new
generated tools, and Part B's after the two planners (§4.3, §6.3).

**Resolved before implementation (N1, on the brief's Q1–Q8 and on `critic-brief.md`):**

- **R1 (C1).** `set_track_mix` **merges** the stored entry's curves into the fresh entry before
  validation and before `is_neutral`; the strip's `Reset` clears the five scalars and **keeps** the
  curves — it is "return to unity", not "delete my ride" — with the tooltip amended and a
  `reset_row_keeps_automation` test (§2.5, §5.3, A5, B7).
- **R2 (C2).** `clamp_project_curve(curve, new_duration)` is called from `recompute_duration` over
  the four new project-frame owners **and** the existing bus/master `Effect.keyframes`, fixing
  AU2's latent trap in the same line. §5's `untouched` cells become "clamped when the project
  shortens" (§2.3, §2.4, A7).
- **R3 (C3).** A `Hold` segment is flat: `automation_step` is segment-aware (`v1 = v0` on a `Hold`
  segment), the value changes at the next key's **first sample**, and the step is declicked
  **forward** over `min(5 ms, segment length)` (§3.1, A9).
- **R4 (C4).** `current` means "the value last applied", written back once per sample frame by the
  automated path; `ramp_start = current` at retarget; `pending_ramp` is set **only when the
  document inputs changed**, comparing `(audible, gain_tenth_db, pan_percent, gain_curve,
  pan_curve)` for a track stage and `(gain_tenth_db, gain_curve)` for a `GainRamp`, so AU1's
  "an unchanged target must not restart the ramp" invariant and its comment survive (§3.3, §3.4).
- **R5 (C5, C17).** D8's invariant is restated precisely — boundary exact, `Linear` within
  ±1 tenth-dB, `Hold` exact, eased segments **re-shaped** on truncation — with the 156 → 250
  counterexample printed by a test, the dedupe-safety clause written down, and both boundary
  formulas spelled out (§2.3, A6).
- **R6 (C6).** Ripple delete is three ordered steps and ripple insert is one; no bus or master
  curve ripples (Q2) (§2.4).
- **R7 (C7).** Both lerps are written `a + (b − a) * t`, pinned bit-identical to the static path at
  `t = 0`; the constant-power centre is `FRAC_1_SQRT_2`, not `1.0`; the chord bound is stated as
  `1 − cos(Δ·π/400)` (§3.1, §3.7).
- **R8 (C8, Q8).** `LiveAudioChange { None, Mix, ClipShaping, Both }`; `is_clip_audio_operation` is
  split out; `Playback::update_clip_shaping`, the engine control arm and
  `AudioMixer::update_clip_shaping` are **Part A**. "No UI" in Part A's description means no new
  editing surface, not "no app code" (§3.8, §4.4).
- **R9 (C9).** `curve: Option<AutomationCurve>` is kept — one tool per owner — **with**
  `#[schemars(required)]`, justified in §2.5 as the workspace's first use and as a deliberate
  deviation from the two-variant `SetEffectKeyframes`/`ClearEffectKeyframes` precedent; both the
  schema `required` list and the serde omission error are pinned.
- **R10 (C10).** `parameter: String` with `#[schemars(extend("enum" = ["gain_tenth_db",
  "pan_percent"]))]`; the values are pinned against the `render_*` spelling;
  `UnknownTrackAutomationParameter` is kept for hand-edited documents (§2.5, §4.1).
- **R11 (C11).** The `A` chip goes **inline after the kind icon** on `caption_row`; the worst case
  (a `NO AUDIO` track that is also automated) is measured and pinned; the fallback is a tinted
  fader rail (§5.3, B8).
- **R12 (C12, Q5).** Envelope x maps through the clip rect ratio, exactly as the waveform does; no
  band under 24 px of clip width; the envelope interact is allocated **after** `body`/`left`/`right`
  and intersected with `body_rect` horizontally (egui 0.35.0 resolves ties by
  last-registered-wins, `hit_test.rs:429-433`); the display range is −40 … +12 dB linear in
  tenth-dB with no fine-drag
  modifier (§5.1).
- **R13 (C13).** `plan_audio_ducking` keys the curve **relative to the stored scalar**, refuses an
  existing `gain_curve` unless `replace: true`, states the ≥ 400 ms measurement floor and the
  ≥ 950 ms duck-window fixture rule, and reports `measured: null` with a reason rather than
  refusing the plan when no window is long enough (§6.1).
- **R14 (C14, Q3).** Reject → clamp is unified in Part A for clip-local **keyframes and audio
  fades**; a *written* over-running curve still fails, pinned by a new test; both changes get a
  CHANGELOG line (§2.4, §1.4).
- **R15 (C15).** Whole-owner-set semantics for `UpsertAudioBus`/`SetAudioMaster` — an omitted
  `gain_curve` clears it, exactly as an omitted `effects` clears the chain — are named and
  accepted, and made recoverable by `render_*` spelling `gain_curve=[…]` so the agent can read
  before it writes; a round-trip test pins it (§2.5, §4.1).
- **R16 (C16, Q4).** `ClipAudioShaping` loses `Copy` and `gain_at` takes `&self`; the per-segment
  `Vec<Keyframe>` clone is once per clip per export, not per sample. `TrackMix` loses `Copy`;
  `track_mix_operation` loses `const` (C18) (§2.1, §3.2).
- **R17 (C19).** §4.3's byte figures are labelled **estimates to be measured and re-derived**; the
  growth term is named as *field* growth on four `Operation`-reachable types, not a new `$defs`
  type.
- **R18 (C20).** Evidence (b) is restated as two asserts: bit-identity at every project frame's
  **first sample** against `db_gain(value_at(f))`, and monotonicity between anchors on the interior.
- **R19 (C21).** `SetClipGainEnvelope` rejects `Title`/`Freeze` with the existing two errors, and
  `Some(empty)` becomes `InvalidClipGainEnvelope` through `AutomationCurve::validate`'s `Empty`.
- **R20 (C22).** The `SlipClip` row's audible consequence is stated and "slip-following envelopes"
  is named in §1.3 beside "speed-following envelopes".
- **R21 (Q1/D7).** The automation key stays the **input** frame; the offset is pinned rather than
  pretended away as `stage_latency_frames(bus) + stage_latency_frames(master)`, exactly 0 without
  declared lookahead; the clip envelope and the track stage share the offset, so their *relative*
  alignment is exact (§3.5).
- **R22 (Q6).** Both planners; `plan_clip_fades` is the one to cut if Part B runs hot (§6.2).
- **R23 (Q7).** The critic's version: the fader and pan **rail** are disabled while a curve exists;
  the numeric readout stays editable and writes the parked scalar under CC5's rule. One DESIGN.md
  sentence for each rule (§5.3, §5.6).
- **R24 (dropped items).** `get_audio_levels`' output gains the two `Option` fields on
  `TrackLevels.mix`, default-omitted, with a golden pair for a curve-bearing document (§4.2);
  CHANGELOG carries a line for each part and one for each reject → clamp change (§1.4); `qa_document`
  and `get_audio_qc` gain **no** automation-aware finding, and §4.6 says so.

**Resolved before implementation (N2, on `critic-contract.md` F1–F29 and on its rulings on
revision 1's four open notes, which are now closed).** Every finding is folded into the rule, cell
or item it belongs to; these entries record where each landed. Where an N2 ruling amends an N1 one,
it says so.

- **R25 (F1).** The latency offset is **per owner**, not a graph sum: clip envelope and track stage
  **0**, the bus fader early by its chain's own node latency `ℓ_bus`, the master fader early by
  `stage_latency_frames(L_bus)`. Amends R21's single figure; §3.5 rules 65 and 66 are rewritten and
  A14 prints all four as `AU4_LATENCY owner=… declared_ms=… offset_frames=…`.
- **R26 (F2).** `recompute_duration` clamps **only when `duration < previous`**. The guard is
  load-bearing, not an optimisation: it is what keeps rule 20's invariant arm reachable and what
  keeps `au2_core.rs:1128-1151`'s `AudioMasterKeyframeOutsideProject` pin green (§2.3 rule 18,
  §2.4 rule 20).
- **R27 (F3).** `new_duration <= 0` reduces a project curve to a single key at frame 0 valued
  `value_at(0)`, and the four project-frame bounds **and** the existing bus/master check become
  `at < duration || (duration <= 0 && at == 0)`, so deleting the **only** clip of an automated
  project succeeds instead of being rejected (§2.3 rule 17, §2.6 rule 37.3, A7).
- **R28 (F4).** The retarget tuple includes **`pan_law`**, because `track_stage_parameters` reads it
  and `SetPanLaw` takes the live path; without it a law change becomes a silent no-op on every
  track during playback (§3.3 rule 59, A15).
- **R29 (F5).** `delta_local` is **signed**: a left trim-out, a left slide and a left roll make it
  negative, step 1 inserts nothing on the left, and the newly exposed head is flat at the first
  key's value. Rule 16's boundary-exact clause is scoped to **shortening** edges (§2.3 rules 13
  and 16).
- **R30 (F6).** The declick's `w >= 1.0` arm returns `target` **exactly** — `h + (target − h)` is up
  to ≈ 500 ulps away on the −60 dB duck §6.1 designs for — so there is no seam to bound; rule 45
  asserts `to_bits()` identity and interior monotonicity instead of a tolerance (§3.1 rules 42 and
  45, A9).
- **R31 (F7).** The anchor memo is **per curve**, keyed `(frame_index, curve_epoch)`, with
  `curve_epoch` a new `u32` on `TrackStageRuntime`, `GainRamp` and `ClipAudioShaping` bumped
  unconditionally on retarget or rebuild — none of the three carries `parameter_epoch` (§3.1
  rule 46, A15).
- **R32 (F8).** The ducked floor is flat over exactly `[a, b]`, so a measurement window fits when
  `span + hold >= 400 ms` — 200 ms of speech at the default hold — and an un-ducked window needs a
  gap of `release + 400 ms`. Amends R13's 950 ms fixture rule (§6.1 rule 128.2, B11).
- **R33 (F9).** `set_track_automation` shares `set_track_mix`'s entry lifecycle — insert, replace,
  remove when neutral, re-sort by track — so clearing the last curve leaves the document
  byte-identical to one that never carried it (§2.5 rule 32a, A5).
- **R34 (F10).** The `SlideClip` and `RollEdit` cells carry the signed formulas instead of "each
  neighbour `E` on its changed edge": only the right neighbour's `timeline_start` moves, so only it
  has a non-zero `delta_local` (§2.4 table).
- **R35 (F11).** `RelinkAsset` gets its own survival row — `E(0, new_duration)` plus rule 26.1's
  fade clamp on every clip whose project duration changed — because a candidate asset with a
  different `fps` re-derives `clip_duration` (§2.4 table, rule 20, A8).
- **R36 (F12).** One pure `envelope_band_rect(clip_rect, kind) -> Option<Rect>` is used by paint,
  hit and interact, so the drawn key and the grabbed key cannot disagree; a Video, Title or Freeze
  clip has no band and no interact (§5.1 rule 95a).
- **R37 (F13, and the ruling on open note 2).** The strip's worst case is `NO AUDIO` **and**
  `SILENCED` **and** off-neutral **and** automated. `<= 240.0` is the only binding assert, the `232`
  pin is deleted, the measured figure is recorded as a §0 erratum, and a **second** fallback is
  named: suppress the chip on a `NO AUDIO` strip. Amends R11's case (§5.3 rule 108, B8).
- **R38 (F14).** §8's per-file counts are corrected (`app.rs` **0**, `mixer_ui.rs` **16**,
  `operation.rs` **1**, `au1_core.rs` **2**, plus the two missing files, media `audio.rs` **2** and
  agent `render.rs` **2**) and rule 4 reframes all of them as grep counts that **over-count**
  (§2.1 rule 4, §8).
- **R39 (F15).** The `Copy` losses name their consequences: `AudioMix::track`'s `.copied()` becomes
  `.cloned()`, every `TrackMix`-by-value parameter in `mixer_ui.rs` becomes `&TrackMix` or clones
  at the call site, and `Document::track_mix` is cited at model.rs:1122-1124 (§2.1 rule 5).
- **R40 (F16).** `skip_ramp` on a curve-bearing stage completes to `automated_target(n_end)`, so
  rule 58's definition of `current` holds across gaps and an edit made during a gap cannot ramp
  from the parked value (§3.3 rule 60, A15).
- **R41 (F17).** Every normative rule the checklist missed gains an item — rules 24, 27, 34, 35,
  38, 52, 77, 91–93, 102, 103, 105, 119 and 127 — including a **new** telemetry item **A16** for
  §4.5, which renumbers the old A16–A20 to A17–A21. A3's grep wording is fixed and B1's 27.5
  tenth-dB/px gets a `±0.1` tolerance (§7).
- **R42 (F18).** A speed **increase** is destructive — the keys past the new, shorter duration are
  dropped and only undo restores them — stated in rule 25, given its own `### Changed` CHANGELOG
  line, and pinned by an A8 round trip (§1.4, §2.4 rule 25, A8).
- **R43 (F19).** `reset_row`'s early return is **replaced** by the scalar test, not doubled: under
  the widened `is_neutral` of rule 11 the original branch is unreachable as a distinct arm (§2.5
  rule 33, B7).
- **R44 (F20).** `keyframe_row` is generalised to
  `(ui, key: &str, curve, index) -> KeyframeRowAction` and the inspector's existing caller is
  rewritten onto it in the same commit, because a bus or master fader has no `ClipId` (§5.4
  rule 114).
- **R45 (F21).** `range` is **kept**, with stated semantics: it restricts which dialogue spans are
  considered and clamps the emitted curve, and never restricts the measurement. It is echoed in
  structured content and tested (§6.1 rules 126 and 129, B11).
- **R46 (F22).** `next_key` is defined as the frame of the key **following** the one at `at`, and
  both extra `min` terms in `D` are named **defensive** — they are dead at every supported rate and
  are written down so the formula survives a faster project (§3.1 rule 41.6).
- **R47 (F23).** `set_effect_keyframes` joins the `.idempotent(…)` list beside the two new tools,
  and the annotation delta is measured with the rest (§4.3 rule 84, A19).
- **R48 (F24).** The two live-audio controls collapse into one
  `Control::UpdateAudio(LiveAudioChange, Arc<Document>)`; the worker applies mix then shaping from
  the **same** document, which is the real reason the fixed order is safe (§3.8 rule 75, §4.4
  rule 89).
- **R49 (F25–F29, the nits).** The **left** boundary key's interpolation is named as the
  load-bearing one and the right one's as never read; the automated multiply order is pinned as
  `gain * pan[ch]`; A9 asserts `measured <= sagitta <= bound`; rule 26.1 says it clamps against the
  **project** duration unlike the title clamp beside it; and every stale citation is corrected —
  `CHAIN_LOOKAHEAD_MILLISECONDS` model.rs:645, `AudioMix::is_empty` model.rs:699-704,
  `Document::track_mix` model.rs:1122-1124, `rounded_div` automation.rs:122-128, `set_clip_audio`'s
  Title/Freeze arms operation.rs:3179-3183, `matte_hit_test` (preview_ui.rs:15, called at :1157),
  `AudioStream::fill` audio.rs:2656-2668, the live fader call site audio.rs:2473, and egui 0.35.0's
  `hit_test.rs:429-433`.
- **R50 (the ruling on open note 1).** Rule 48 stands as written — the ruled formula is the bound,
  the sagitta beside it is the exact worst case, and the measured sweep carries the weight — with A9
  asserting `measured <= sagitta <= bound` rather than an equality (§3.1 rule 48, A9).
- **R51 (the ruling on open note 3).** §6.2 ships as written, with `LOUDNESS_GATING_BLOCK_FRAMES`
  named in **sample** frames (19 200 = 400 ms at 48 kHz = **12 project frames** at 30 fps) and a
  clip shorter than the window skipped with a per-clip reason rather than failing the whole plan
  (§6.2 rule 131, B12).
- **R52 (the ruling on open note 4).** The `AUTOMATION` section's budget is **120 px**; the
  `LOUDNESS` note is corrected to "86 px against its own 90 px budget"; and B9 gains a
  **pane-level** `BUDGET` assert on a master pane that also carries `LOUDNESS` (§5.4 rule 116, B9).

No OPEN note remains.

**Implementation errata (Part A core, 2026-09-09):**

- **E1. Rule 28's serde premise is false.** serde's derive does **not** reject an omitted bare
  `Option<T>` field: `missing_field` succeeds through `deserialize_option`, so an omitted `curve`
  would have cleared silently. Both variants carry
  `#[serde(deserialize_with = "deserialize_required_curve")]` (no `default`): omission is
  `missing field \`curve\``, `null` still clears, serialization is unchanged. `#[schemars(required)]`
  stays, and the A1 wire test pins both halves.
- **E2. A ripple insert can over-run an unchanged duration.** A ripple on tracks that do not carry
  the project's last clip shifts keys right without lengthening the project; `ripple_insert_curve`
  therefore clamps the shifted curve into the post-shift duration (`clamp_project_curve` is the
  identity when nothing over-runs), so rule 20 holds.
- **E3. A ripple delete can empty a curve** (`start == 0` and every key inside the removed window);
  rule 22 gains rule 13 step 5's fallback: one key at `start` valued `value_at(ripple_point)`.
- **E4. The `RelinkAsset` row is currently unreachable**: `relink_asset` rejects any fps / duration /
  resolution mismatch with `RelinkMetadataMismatch`, so no bound clip's project duration can change.
  The row is implemented anyway and its test asserts the verbatim outcome.
- **E5. The audio-fade clamp also runs on `slide_clip`'s two neighbours, `roll_edit`'s two clips and
  `set_clip_speed`**, not only on trim and split: those shorten a clip too and rule 20 forbids
  failing. Strictly fewer failures; no existing pin covers those rows.
- **E6. Bus/master fader-curve rejections reuse existing variants**: structural failures map to
  `InvalidEffectAutomation { effect: "audio_bus" | "audio_master", name: "gain_tenth_db" }` (rule
  36's precedent) and out-of-range values to the existing `Audio{Bus,Master}GainOutOfRange`; only
  the two `…GainKeyframeOutsideProject` variants are new.
- **E7. `clamp_project_curve` inserts its boundary key only when a key over-runs**, which is what
  rule 18's "byte-identical when nothing shortened" requires; rule 17 read unconditional.
- **E8. A20's QC-unchanged test lives in `tests/au4_core.rs`**, on the public `qa_document`.
- **E9. `rebase_clip_curve` on `new_duration <= 0`** reduces to one key at 0 valued
  `value_at(delta_local)`, mirroring `clamp_project_curve`; unreachable, since `ZeroProjectDuration`
  rejects such clips.

**Implementation errata (Part A app, 2026-09-09):**

- **E10. Core owns `LiveAudioChange` and `Playback::update_audio`.** §4.4 placed the enum in the
  app; `Control::UpdateAudio` carries it across the crate boundary, so it lives in core `media.rs`
  (`pub`, derives `Default` = `None`) and the app imports it. The `Both` case crosses the control
  channel as ONE call: a defaulted `Playback::update_audio(change, doc)` (default dispatches to
  `update_audio_mix` / `update_clip_shaping` / both / `set_document`), implemented on the engine as a
  single `publish_live_audio(change, doc)`; the app's consumer calls it once.
- **E11. `apply_live_audio_change(playback, change, doc, position) -> bool`** is extracted from
  `poll_background` because `KinewrightApp::new` needs a live GPU engine; it is the seam the
  counting-double test drives, and `poll_background` clears `playing` when it returns `false`.
- **E12. `operation_status` renders `gain` / `pan`** through `track_automation_label(parameter)`
  rather than the wire spellings; an unknown key falls through unchanged because the status line
  also previews unvalidated agent proposals.
- **E13. `mixer_ui::RESET_TOOLTIP`** is a const so the tooltip is assertable (`on_hover_text` leaves
  no rect to probe); `reset_row`'s early return is replaced, not doubled (rule 33).
- **E14. `reset_row`'s behaviour and `reset_row_keeps_automation` land in Part A** (§8 filed them
  under Part B): the widened `is_neutral` would otherwise draw a button the no-op filter swallows.

**Implementation errata (Part A agent, 2026-09-09):**

- **E15. The render goldens are a new AU4 pair** beside the AU2 pair rather than edits inside the
  AU2 test bodies, so the AU2 tests keep pinning AU2's own semantics; rule 82's two goldens exist.
- **E16. A18's "byte-unchanged" is asserted structurally**: the AU1 golden stays green, neither key
  is present without a curve, and stripping the two keys from an automated report's `mix`
  reproduces the curve-free one. A stored report literal would pin media's measured levels, which
  §3 changes.
- **E17. M36 gains the customary pair of rows** (registry + served), as every prior slice's entry.
- **E18. `track_automation_render_key` is on the render path**: `track_mix_fields` looks the key up
  from the wire token, so rule 81's mapping is load-bearing; the `set_track_automation` description
  is built from `TRACK_AUTOMATION_PARAMETERS`, so a rename cannot drift the published vocabulary.
- **E19. Rule 84's annotation flip shrinks the registry by 1 B** (`"idempotentHint":true` is one
  byte shorter than `false` on `set_effect_keyframes`); the derivation comment and M36 state it, so
  the serialized split sums exactly: +93,394 = 48,444 own + 44,951 shared − 1.
- **E20. Measured Part A registry** (re-measured after E21): 54 generated / 78 inspectors / 132;
  serialized 1,519,052 B; input 1,386,288 B (+90,829 = 45,878 for the two tools' own schemas + 53 ×
  820 B of shared field growth on the 52 generated tools and `apply_edit_plan` + 1,491 B of inline
  `oneOf` variants); descriptions 111,103 B (+2,228 = 1,003 + 1,225); served quad unchanged. The 820
  B per tool is rule 85's reuse claim measured, against AU2 Part B's 1,195 B for two new
  definitions.
- **E21. `#[schemars(required)]` alone strips the null branch.** schemars 1.2.2's `required`
  switches an `Option<T>` field to its non-optional schema, so the published `curve` schema forbade
  the very `"curve": null` that clears a curve (rule 28's "both halves pinned" held only for the
  `required` list). Both fields are `#[schemars(required, with = "RequiredNullableCurve")]`, a
  `JsonSchema` helper publishing `anyOf [AutomationCurve, null]`, beside E1's
  `deserialize_required_curve`; the load-bearing sentence ("`null` clears it; the field is
  required") sits on the `curve` FIELD doc, which survives into the tool schema, and each tool's
  first sentence carries the essentials because `get_capability` publishes only that. The A1 agent-
  half test asserts the null branch. The registry figures in E20 are re-measured after this fix (see
  the ledger comment for the final split).

**Implementation errata (Part A media, 2026-09-09):**

- **E22. `skip_ramp` gains `start_sample: u64`** (rule 60 said "keeps its signature" while requiring
  `current = automated_target(n_end)`, which needs the sample index); `channels` is stored on the
  runtime and dropped from `TrackStageRuntime::apply`/`retarget`'s parameters (one source of truth).
  A settled curve-free stage still early-returns unchanged.
- **E23. The gated-track invariant is kept on the automated arm**: `!audible && settled` fills exact
  `0.0` rather than multiplying by `0.0` (`-0.5 * 0.0 == -0.0` is a different bit pattern), so A13's
  byte identity holds on a silenced automated track.
- **E24. Rule 59's unchanged arm leaves an in-flight ramp alone** rather than forcing
  `ramp_index = ramp_frames`. AU1 could force it because its condition was `target == current`, so
  settling was a no-op; under the document-tuple comparison an unchanged owner caught mid-ramp
  (every other track when one fader moves) would settle at the interpolated value forever.
  `ramp_frames` therefore drops out of the four `retarget` signatures. Pinned by
  `an_unchanged_document_tuple_lets_an_in_flight_ramp_finish`.
- **E25. `LiveAudioChange` is `pub` in core `media.rs`** (see E10) and **`Playback::update_audio`**
  is added there, defaulted; the engine overrides it as one `publish_live_audio` send and
  `Control::UpdateAudio(LiveAudioChange, Arc<Document>)` replaces `Control::UpdateAudioMix`.
- **E26. `ClipAudioShaping`'s anchor memo is a `Cell`** because rule 50 pins `gain_at(&self)`;
  nothing here crosses a thread (rule 76). It also carries `sample_rate` / `project_fps`.
- **E27. Rate and fps are stored on the runtimes**: `TrackStageRuntime::new` gains `sample_rate`;
  `GainRamp::settled`, `AudioBusRuntime::new` and `AudioMasterRuntime::new` gain `project_fps` — the
  automated arms need the sample→frame conversion the contract's signatures did not carry.
- **E28. The `AU4_PAN_CHORD` sweep measures the chord-versus-arc geometry in `f64`**: in `f32` at
  Δ = 1 the sagitta (7.71e-6) is within a few ulps of the anchors' own representation error
  (6.0e-8 near 0.707), so the assert cannot hold in `f32` and the excess is not the chord. The
  shipped `f32` `automated_pan` is separately asserted within `8·f32::EPSILON` of the same chord at
  every Δ and start position.
- **E29. `AudioMixer` stores `decode_from` and `output_rate`** so `update_clip_shaping` re-derives
  exactly the segment list `open` built (a mixer opened without preroll starts at `project_from`,
  not 0).
- **E30. A13's "byte-identical to pre-AU4" is discharged structurally** (pre-AU4 code is not
  runnable in-tree): a curve-free document and the same document with a one-key constant curve on
  all five owners export `to_bits()`-identically over the whole buffer (0 of 288 000 samples
  differ), plus `gain_at` identity against the explicit pre-AU4 product, the fast-path assertions,
  and a printed SHA-256 of the curve-free mix.
- **E31. Measured (48 kHz, 30 fps):** `AU4_HOLD_STEP boundary=160000 reached=160239 exact=160240`;
  `AU4_HOLD_SEAM bits_equal=true`; `AU4_LATENCY` at 20 ms declared: clip envelope 0, track gain 0,
  track pan 0, bus fader 960, master fader 960 (all 0 at 0 ms); `AU4_PAN_CHORD` Δ=10 measured
  7.70e-4 against sagitta 7.71e-4 and bound 3.08e-3.

**Implementation errata (Part A manifest, 2026-09-09):**

- **E34. The shaping layout check also compares the asset path** (a sixth condition beside rule
  73's five), returning `false` — the safe direction, a re-cue — when a relink changed the file
  under an otherwise identical segment list; unreachable on the live path (rule 74) and recorded
  as defence in depth. `Worker::update_clip_shaping` carries the same `can_retarget_audio_mix` guard
  as the mix arm; under `Both` it compares the document with itself and cannot re-cue twice.
- **E33. Rule 37's validation order is per keyframe, not per pass.** The three curve validators check
  value range and then the bound inside one `for keyframe` loop, so a curve whose first key is out
  of bounds in `at` and whose second is out of range in `value` reports the bound error where the
  literal rule would report the range error. All three validators agree with each other; the
  structural check (rule 37.1) still runs first as a whole.
- **E32. §8's Part A app manifest under-counts the mechanical sites.** Beyond `app.rs`, `mixer_ui.rs`,
  `inspector_ui.rs` and `timeline_ui.rs`, the `audio_gain_curve: None` / `gain_curve: None` literals
  and the two borrow adjustments forced by `TrackMix`'s `Copy` loss also touched the other app files
  `git diff --stat -- crates/kinewright-app` lists at commit time; the counts in §8 are grep bounds
  (rule 4), not edit sites.

## 1. Scope

### 1.1 The editor job

The editor from AU3 has a balanced, processed, loudness-verified mix. Every audio control they own
is a *constant*: one gain per clip, one gain and pan per track, one fader per bus and master. They
cannot duck the music under a line, ride a fader through a noisy passage, pull one loud word down,
or make a hard cut stop clicking. AU4 gives every one of those controls a time axis, gives the clip
a rubber band on the timeline, gives the mixer an automation editor, teaches every editing operation
what to do with a curve it moves, and gives the agent a ducking planner that proposes one and proves
it with a measurement.

### 1.2 The two parts

**Part A — Automation model and evaluation.** The five new curve fields (§2.1); the resolution and
neutrality rule (§2.2); `rebase_clip_curve` and `clamp_project_curve` (§2.3); the survival policy in
every clip-moving operation, including audio fades (§2.4); `SetClipGainEnvelope`,
`SetTrackAutomation`, the `set_track_mix` merge and the nine new `OpError` variants (§2.5, §2.6);
`automation_step` and its `Hold` declick (§3.1); the clip envelope inside `ClipAudioShaping` (§3.2);
the automated track stage and the automated bus/master faders (§3.3, §3.4); the pinned latency
offset (§3.5); `update_clip_shaping`, `LiveAudioChange` and the engine arm (§3.8, §4.4); the
`render_*` spellings, `get_audio_levels`' two new fields and the registry ledger (§4.1–§4.3). **No
new editing surface**; the app work is the live-path predicate and the `Playback` method. Commit
`feat: complete AU4a automation model`.

**Part B — Automation editing and planners.** The timeline rubber band and the timeline's first
coalescing path (§5.1, §5.2); the Mixer `A` chip, the disabled-rail rule and the chain pane's
`AUTOMATION` section (§5.3, §5.4); the inspector `ENVELOPE` block (§5.5); DESIGN.md (§5.6);
`plan_audio_ducking` and `plan_clip_fades` (§6.1, §6.2); the Part B registry ledger (§6.3). Commit
`feat: complete AU4b automation editing`.

**How the roadmap row is discharged.** The AU4 row's **exit gate** (ROADMAP-AND-WORKFLOWS.md:648)
has two clauses. (1) *"Envelope evaluation is integer-exact and identical in both paths"* — closed
entirely by **Part A** (A9–A13). (2) *"edited envelopes survive split/trim/slip/speed operations
under a stated policy"* — closed entirely by **Part A** (A6–A8). The row's **deliverables**
— *"Keyframed clip gain envelopes with a rubber-band editor on the clip, track gain and pan
automation, bus automation editing in the mixer, audio-aware trim behaviour, agent planners that
propose envelopes"* — split across the two: the *model* of clip envelopes and track automation and
the whole of audio-aware trim behaviour are Part A; the rubber-band editor, bus automation editing
in the mixer and the planners are Part B. Part A also discharges AU2's own deferral *"Automation
editing UI for bus and master parameters (AU4; the data model already carries the curves)"*
(AU2-EQ-AND-DYNAMICS.md:432) by *model*, and Part B by *surface*; and it pays off AU1's clip-gain
re-cue debt (§3.8).

### 1.3 Out of scope (named deferrals)

Full automation lanes on the timeline (a track's curve drawn in its own lane rather than on the
clip). Write, touch and latch modes, and any "record a fader move while playing" — AU4 is **read
mode only**. Bezier interpolation and per-key tangents; the five `KeyframeInterpolation` kinds are
the whole vocabulary, and their absence is exactly why an eased segment is re-shaped by a truncating
trim (§2.3). **Speed-following envelopes** (§2.4 row `SetClipSpeed`) and any audible envelope on a
retimed clip while retimed audio stays muted. **Slip-following envelopes** (§2.4 row `SlipClip`).
Tempo-synced or beat-quantised ducking; signal-derived (sidechain-detector) ducking, which is AU2's
`audio_ducking` node and not automation. A per-sample automation time base (§3.1). Bus pan
automation (there is no bus pan, AU2 §1.3). Automation of `PanLaw`, mute or solo — switches, not
rides. Per-keyframe operations (`AddKeyframe`/`MoveKeyframe`/`RemoveKeyframe`). A fine-drag modifier
on the rubber band (Alt is already the snap bypass, timeline_ui.rs:367). Automation on video clip
opacity. Latency-compensated automation alignment (§3.5). Envelope-authoring planners beyond
ducking (per-clip level rides, tail fades on envelopes). A curve cursor for `value_at`'s O(n) scan
(automation.rs:87-90) — a performance deferral to revisit only when a real project makes it
measurable. No repair, noise profile, room tone or de-click *process*: those are AU5
(ROADMAP-AND-WORKFLOWS.md:649); the declick this contract specifies is an envelope boundary rule,
not an algorithm. No workflow definition, prompt or blind review: those are AU6 (:650).

### 1.4 Documentation

**Part A:** this contract; `CHANGELOG.md` — one `### Added` line for the five curve owners and the
two operations, and **three** `### Changed` lines, one for "a trim, split, roll, slide or speed
change that shortens a clip past a keyframe now clamps the curve instead of failing with
`EffectKeyframeOutsideClip`", one for "a trim or split that would over-run a clip's audio fades
now clamps them instead of failing with `AudioFadesTooLong`", and one for "increasing a clip's
speed now drops the envelope keys past the clip's new, shorter duration; only undo restores them"
(§2.4 rule 25), plus a `### Fixed` line for "a split
no longer slides the right half's clip-local automation curves" and one for "shortening a project no
longer rejects the edit when a bus or master effect curve keys past the new duration";
`MEDIA-POLICY.md` "Playback audio mixdown" (:106-141) gains one paragraph on `automation_step`, the
shared per-sample ramp and the input-frame automation key; `AU1-MANUAL-MIX.md:309-321` and AU2
§2.2's keyframe rules gain AU4 pointer sentences in AU1 §0 style (text not rewritten);
`M36-AGENT-RUNTIME-EFFICIENCY.md:95-116` gains a Part A row.

**Part B:** this contract; `CHANGELOG.md`; `ROADMAP-AND-WORKFLOWS.md` status paragraph names the two
parts and the clause split; `DESIGN.md` `### Timeline` and `### Mixer` (§5.6); `README.md` line 36
gains clip envelopes and track automation; `M36` Part B row.

---

# Part A — Automation model and evaluation

---

## 2. Part A core model

### 2.1 The five curve fields (D1, C16, C18, Q4)

Five new fields, all `Option<AutomationCurve>` with the standing optional-field attributes
(N0): `#[serde(default, skip_serializing_if = "Option::is_none")] #[schemars(default)]`.

| owner (file:line) | field | time base | unit and validated range |
| --- | --- | --- | --- |
| `Clip` (model.rs:298-346) | `audio_gain_curve` | **clip-local** frames | tenth-dB, `-600..=120` |
| `TrackMix` (model.rs:509-527) | `gain_curve` | project frames | tenth-dB, `-600..=120` |
| `TrackMix` | `pan_curve` | project frames | percent, `-100..=100` |
| `AudioBus` (model.rs:490-507) | `gain_curve` | project frames | tenth-dB, `-600..=120` |
| `AudioMaster` (model.rs:571-581) | `gain_curve` | project frames | tenth-dB, `-600..=120` |

1. **Normative.** A curve lives on the owner whose value it drives. No document-level automation
   table exists: `split_clip` and `replace_clip` clone the whole `Clip` (operation.rs:1637-1686,
   :2277-2358), so clip-anchored data held beside the clip is silently lost by every existing and
   future clone-the-clip operation.
2. **Normative.** `Clip.audio_gain_curve` is keyed in **clip-local** frames — `local = project_frame
   − clip.timeline_start`, the keying the video path already uses (timeline.rs:209-211) — and the
   other four in **project** frames, which is exactly the split `Keyframe::at`'s doc comment already
   declares (automation.rs:24-25) and the split between `SetClipAudio` and `SetTrackMix`.
3. `Clip.audio_gain_curve` is placed after `audio_fade_out_frames` and before `speed_percent`;
   `TrackMix.gain_curve` after `pan_percent` and `pan_curve` after it, both before `mute`;
   `AudioBus.gain_curve` after `gain_tenth_db`; `AudioMaster.gain_curve` after `gain_tenth_db`.
   Field order is wire order for a struct with no `rename_all`, and every existing golden is
   byte-unchanged because every new field is absent by default.

**Costs this accepts, named in full.**

4. **Literal sites.** Neither `Clip` nor `TrackMix` has a `Default`, so every struct literal gains
   the field: **140** `Clip {` sites, **27** `TrackMix {`, **47** `AudioBus {`, **25**
   `AudioMaster {`. These are **grep counts** — workspace-wide, word-bounded, `target/` excluded,
   re-counted on de0beb3 — and they **bound the work while over-counting it**: each total includes
   the `pub struct X {` definition itself, and **15 of the 27** `TrackMix {` occurrences are
   followed by `..` struct-update syntax and need no edit at all. §8 enumerates them per crate per
   part on the same basis.
5. **`TrackMix` loses `Copy`**, keeping `Clone + PartialEq + Eq`. `Document::track_mix`
   (model.rs:1122-1124) returns a clone; it is called from `track_stage_parameters`
   (audio.rs:1553-1568) at processor construction and retarget only, from `measure_mix_levels`
   (export.rs:1453-1462) once per track per measurement, and from the app's mixer per frame.
   `TrackMix::neutral` and `TrackMix::is_neutral` stay `const` (`Option::is_none` is `const`), as do
   `AudioMaster::is_neutral` and `AudioMix::is_empty` (model.rs:699-704), so that doc comment's
   stated reason survives. Two further consequences are named here rather than discovered at the
   compiler: `AudioMix::track` (model.rs:758-764) changes `.copied()` to `.cloned()` — it is the
   hot read behind `Document::track_mix`, `track_stage_parameters` and `measure_mix_levels` — and
   every `TrackMix`-by-value parameter in `mixer_ui.rs` (`reset_row` :868 and its siblings) becomes
   `&TrackMix` or clones at the call site.
6. **`track_mix_operation` loses `const`** (mixer_ui.rs:1125-1133): once `TrackMix` owns a `Vec` the
   by-value parameter is dropped at the end of the body. `track_mix_toggle_operation` loses it for
   the same reason. `track_mix_operation_carries_the_whole_mix_state` (mixer_ui.rs:1353) constructs
   a `TrackMix` literal and is one of the 27.
7. **`ClipAudioShaping` loses `Copy`** and `gain_at` takes `&self` (§3.2). It is a by-value field of
   `AudioMixSource` (audio.rs:2204) and is rebuilt per segment inside `mix_pass`'s loop
   (export.rs:1181), so the added `Vec<Keyframe>` is cloned **once per clip per export**, never per
   sample.
8. **`SetClipAudio` does not clobber the envelope.** `set_clip_audio` (operation.rs:3174-3200)
   mutates the clip in place and writes exactly three fields, so a clip-gain drag leaves
   `audio_gain_curve` alone. `set_track_mix` (operation.rs:1194-1218) *constructs a fresh entry*
   and is the asymmetric case; §2.5 rule 9 fixes it.

### 2.2 Resolution and neutrality (D2)

9. **Normative.** A curve **replaces** its scalar; the scalar is the parked value. Resolution is
   exactly `Effect::integer_parameter_at`'s rule (model.rs:214-222): curve first, static second.
   There is no multiplicative trim, no second gain stage, and no product that can exceed the
   +12 dB ceiling.
10. **Normative.** With no curve the owner's code path is the pre-AU4 one: `automation_step` is
    never called, the fast paths in `TrackStageRuntime::apply` (audio.rs:1697-1703) and
    `GainRamp::apply` (audio.rs:1488-1495) still fire, and a curve-free document exports
    **byte-identically** to pre-AU4 (A13).
11. `TrackMix::is_neutral` gains `&& self.gain_curve.is_none() && self.pan_curve.is_none()`;
    `AudioMaster::is_neutral` gains `&& self.gain_curve.is_none()`. A neutral-scalar owner carrying
    a curve is therefore never elided off the wire and never removed by the `retain` arm of
    `set_track_mix`.
12. `value_at` returning `None` (i64 overflow only, automation.rs:102) falls back to the **static
    scalar**. It never panics and never clamps silently to a bound the editor did not write.

### 2.3 `rebase_clip_curve` and `clamp_project_curve` (D8, C2, C5, C17)

Two pure core functions in `automation.rs`, re-exported from `lib.rs`:

```rust
/// AU4 §2.3: move a clip-local curve to a new local origin and duration.
///
/// The value at the new boundary frame is preserved exactly; interior values are
/// preserved to within one tenth-dB on `Linear` segments and exactly on `Hold`
/// segments, and are **re-shaped inside a truncated eased segment** — an eased
/// curve re-parameterised onto a shorter span is a different curve. Exact eased
/// truncation needs per-key tangents, which are a named deferral (§1.3).
#[must_use]
pub fn rebase_clip_curve(
    curve: &AutomationCurve,
    delta_local: TimeCode,
    new_duration: TimeCode,
) -> AutomationCurve;

/// AU4 §2.3: clamp a project-frame curve into a shortened project without
/// changing the value audible at the new last frame.
#[must_use]
pub fn clamp_project_curve(curve: &AutomationCurve, new_duration: TimeCode) -> AutomationCurve;
```

13. **`rebase_clip_curve`, normative, in this order.**
    1. **Insert boundary keys first, from the *old* curve.** If any key would land at `at < 0`,
       insert a key at local frame `0` valued `curve.value_at(delta_local)`, carrying the
       interpolation of the segment that contains `delta_local`. If any key would land at
       `at >= new_duration`, insert a key at local frame `new_duration − 1` valued
       `curve.value_at(delta_local + new_duration − 1)`, carrying that segment's interpolation.
       **Both formulas are written out because this is the one line the whole policy turns on**:
       the left value is `value_at(delta_local)`, the right value is
       `value_at(delta_local + new_duration − 1)`, and they are not the same expression. The
       **left** key's interpolation is the load-bearing one — the segment containing `delta_local`
       governs the shape to its right, so carrying it is what preserves that shape — while the
       right key's is never read, because it is the last key and `value_at` returns `last.value`
       for every `at >= last.at` (automation.rs:83-86). It is carried anyway, so the two arms are
       one expression.
    2. Shift every surviving key's `at` by `−delta_local`.
    3. Drop keys outside `0..new_duration`.
    4. Dedupe by `at`, later wins, through the `dedup_keyframes` precedent (captions.rs:271-279, a
       `BTreeMap::insert`), so `AutomationCurve::validate`'s strictly-increasing rule holds.
    5. If the result is empty, keep the single boundary key. The envelope becomes a constant equal
       to what was audible at that edge; it **never** falls back to the static scalar, which would
       change the sound.

    **`delta_local` is signed.** Normative. It is negative whenever the head is pulled **out** — a
    left `TrimClip` whose `new_source.start < original.source_range.start`
    (operation.rs:1721-1729), a left `SlideClip` (:2274-2276), a left `RollEdit` (:2194-2196). In
    that case step 1's left arm inserts **nothing**: no key can land at `at < 0`, step 2 shifts
    every key **right** by `|delta_local|`, and the newly exposed head `[0, −delta_local)` is flat
    at the first key's value because `value_at` clamps before the first key (automation.rs:79-82).
    That is the correct policy — the curve is keyed to the material, and material that was never
    under the curve gets the curve's head value — and it is why rule 16's "boundary exact" clause
    is stated **only** for a shortening edge. An implementer must not assert `delta_local >= 0`:
    three of the §2.4 rows reach the negative branch.
14. **Dedupe safety, stated so the rule does not look arbitrary.** A boundary key can only ever
    collide with a surviving key that carries the *same* value and the *same* forward
    interpolation: a key surviving at local 0 means `delta_local` is exactly that key's frame, so
    `value_at(delta_local)` is its own value; likewise at `new_duration − 1`. Last-wins therefore
    cannot pick a wrong value.
15. A **lengthening** trim inserts nothing on the right: `value_at` clamps past the last key
    (automation.rs:83-86), so the tail is already flat at the last value, and `new_duration − 1` is
    the last legal local frame under `at < clip_duration`.
16. **The invariant, precisely** (this replaces D8's doc comment, which promised an exactness the
    three eased kinds cannot deliver): *at the new boundary frame of a **shortening** edge the value
    is exactly what it was; inside a `Hold` segment it is exactly what it was; inside a `Linear`
    segment it is what it was to within one tenth-dB, which is `rounded_div`'s own resolution
    (automation.rs:122-128); inside a **truncated eased** segment it is re-shaped.* The
    boundary-exact clause is deliberately **not** claimed for a lengthening edge or for the head
    exposed by a negative `delta_local`: there the boundary frame did not exist in the old curve and
    `value_at`'s flat clamp is the answer (rule 13). The counterexample is pinned and **printed** by
    a test rather than discovered: old curve `{at 0, value 0, EaseInOut}`, `{at 100, value 1000}`,
    right trim to `new_duration = 50` with `delta_local = 0`. At frame 49 the boundary key is
    **485** in both curves. At frame 25 the old curve reads `linear = 250_000 → eased = 156_250 →`
    **156** and the new curve reads `linear = 510_204 → eased = 515_303 →` **250** — a move of **94
    tenth-dB (9.4 dB)**. The test prints `AU4_EASED_RESHAPE frame=25 before=156 after=250 delta=94`
    beside its assert.
17. **`clamp_project_curve`, normative.** Insert a key at `new_duration − 1` valued
    `curve.value_at(new_duration − 1)` **before** dropping keys at `at >= new_duration`; then drop
    them; then dedupe last-wins; if the result would be empty, keep the single boundary key. When
    `new_duration <= 0` the curve is reduced to its **single boundary key at frame 0**, valued
    `curve.value_at(TimeCode::ZERO)` — a constant equal to what was audible at the head — and the
    four project-frame bounds in rule 37.3 become
    `at < duration || (duration <= 0 && at == 0)`. The **same** relaxation is applied to the
    existing `Audio{Bus,Master}KeyframeOutsideProject` check (operation.rs:4188-4204), because a
    one-key constant curve on an empty timeline is a legal document that today cannot be reached:
    the invariant is `keyframe.at >= duration` with no `duration == 0` guard (operation.rs:4190),
    so at `duration == 0` **every** key — including one at frame 0 — is outside, and deleting the
    only clip of a project carrying a master fader ride is rejected. That is the commonest way to
    shorten a project and it is the case R2 exists to fix.
18. **Call site, normative.** `Document::recompute_duration` (model.rs:1169-1175) is the one place
    `duration` moves. It captures `let previous = self.duration;` before the scan and, **only when
    `duration < previous`**, calls `clamp_project_curve(…, duration)` over every
    `TrackMix.gain_curve`, every `TrackMix.pan_curve`, every `AudioBus.gain_curve`,
    `AudioMaster.gain_curve`, **and every `AutomationCurve` in every bus and master
    `Effect.keyframes`** — the last of which fixes AU2's pre-existing trap in the same line. The
    `duration < previous` guard is **load-bearing, not an optimisation**: it is what keeps rule
    20's invariant arm reachable. `recompute_duration` runs unconditionally on every operation and
    **before** `validate_document` (`ApplyOp for Operation`, operation.rs:427 then :428), so an
    unguarded clamp would silently repair an agent-authored `UpsertAudioBus`, `SetAudioMaster` or
    `SetEffectKeyframes` that keys past the end and make the raise at :4190-4204 unreachable
    through any operation. With the guard, a write that over-runs an *unchanged* duration leaves
    the duration alone, nothing is clamped, and `validate_document` rejects it exactly as it does
    today — which is what `au2_core.rs:1128-1151` pins and what must stay green. A curve already
    inside the new duration is left byte-identical (the function returns a clone equal to its
    input), so no document that did not shorten changes.
19. **Why this is required, not a nicety.** `Document.duration` is derived, not authored; there is
    no `SetProjectDuration`. `DeleteClip`, a right `TrimClip` on the last clip, `RippleDeleteClip`,
    `SetClipSpeed` and `MoveClip` of the last clip all shrink it. Without rule 18, deleting the last
    clip of a project whose music track carries a ducking curve keyed to the end of the dialogue is
    **rejected** with `TrackAutomationKeyframeOutsideProject` — and `plan_audio_ducking` (§6.1)
    produces exactly the curves that would trip it.

### 2.4 Survival policy (D8–D11, C6, C14, §5 of the brief)

`E(delta_local, new_duration)` is rule 13 applied to **both** the clip envelope and **every**
clip-effect curve in `clip.effects` — one policy for one time base (D9). `delta_local` is always
`new_timeline_start − old_timeline_start` in project frames and `new_duration` is always
`doc.clip_duration(clip)?` after the operation's own rewrite, never the source-range span
(`trim_clip`'s local `duration` at operation.rs:1738 is source frames and is not it). In the
neighbour rows below, `d_left` and `d_right` are `doc.clip_duration(x)?` **after** the operation's
own rewrite, and `delta_local` is signed under rule 13.

| operation | clip envelope + clip effect curves | track automation | bus / master |
| --- | --- | --- | --- |
| `MoveClip` (:1749) | untouched — a clip-local curve rides along | untouched | untouched |
| `SplitClip` (:1637) | left `E(0, offset)`; right `E(offset, end − at)` — **fixes the bug** | untouched | untouched |
| `TrimClip` left (:1688) | `E(delta_local, new_duration)`, `delta_local > 0` trimming in | untouched | untouched |
| `TrimClip` right | `E(0, new_duration)` | untouched | untouched |
| `SlipClip` (:2108) | **untouched** — slot and duration unchanged | untouched | untouched |
| `SlideClip` (:2201) | middle untouched (slot and duration both move by the same amount); left neighbour `E(0, d_left)` — only its `source_range.end` moves (:2262-2272); right neighbour `E(to − middle.timeline_start, d_right)`, **signed** and negative on a left slide (:2274-2276) | untouched | untouched |
| `RollEdit` (:2130) | left clip `E(0, d_left)` — only its `source_range.end` moves; right clip `E(to − right.timeline_start, d_right)`, **signed** and negative on a left roll (:2192-2196) | untouched | untouched |
| `ReplaceClip` / `FitToFill` (:2277) | verbatim — the slot is unchanged | untouched | untouched |
| `SetClipSpeed` (:3157) | `E(0, new_duration)` | untouched | untouched |
| `RippleDeleteClip` (:2389) | rides along with `timeline_start` | **shifted**, rule 22 | untouched |
| `RippleInsertGap` (:2418) | rides along | **shifted**, rule 23 | untouched |
| `DeleteClip` (:2383) | removed with the clip | untouched | untouched |
| `RemoveTrack` | — | removed with the track's `TrackMix` (existing purge) | untouched |
| `RemoveAudioBus` (:1471) | — | untouched | the bus's curve goes with the bus |
| `RelinkAsset` | `E(0, new_duration)` on **every** clip bound to the relinked asset whose project duration changed, plus rule 26.1's fade clamp on each — `clip_duration` is derived through `clip_effective_fps(asset.fps, clip)` and `map_source_range_to_project` (model.rs:1147-1161), so a candidate asset with a different `fps` re-derives it | untouched | untouched |
| **any operation that shortens `document.duration`** | — | **clamped**, rule 18 | **clamped**, rule 18 |

20. **Normative.** No trim, split, slip, roll, slide, speed change, ripple, delete or **relink**
    can fail because of automation. `RelinkAsset` is in that list because of its own row above: it
    moves no slot, but it does move `clip_duration`, and it is the one an editor runs on a whole
    project at once. `EffectKeyframeOutsideClip` and the two
    `Audio{Bus,Master}KeyframeOutsideProject` errors survive as **invariants only**: they still
    reject a *written* curve that over-runs its owner — a hand-edited document, an agent-authored
    `SetEffectKeyframes` or `UpsertAudioBus` — and stop failing the editor's edit. `grep -rn
    EffectKeyframeOutsideClip crates --include=*.rs` returns exactly two hits, both in
    `operation.rs` (:869 declaration, :3413 raise): **no test pins *that* rejection**, so the blast
    radius of the change is one new test and one CHANGELOG line. Its two project-frame twins **are**
    pinned — `au2_core.rs:1128-1151` applies a `SetAudioMaster` carrying `Keyframe { at:
    TimeCode(60) }` on a 60-frame project and asserts `OpError::AudioMasterKeyframeOutsideProject {
    at: 60, duration: 60 }`, and `:1287-1297` pins its message — and **both must stay green**,
    which rule 18's `duration < previous` guard is exactly what guarantees.
21. **`SlipClip`'s audible consequence, stated.** A slip drags the material out from under a level
    ride: a dip painted on one word now sits on a different word. This is the correct policy — it is
    what a slip already does to a colour curve, and the curve is keyed to the slot as every
    clip-local curve is — and it is the row an editor will report as a bug, which is why
    "slip-following envelopes" is a named deferral (§1.3).
22. **`RippleDeleteClip`, three ordered steps** on the source track and every `sync_lock` track,
    over `TrackMix.gain_curve` and `TrackMix.pan_curve` only. Let `start = removed.timeline_start`
    and `ripple_point = start + duration` (the removed clip's **end**, operation.rs:2393-2397).
    1. When `start > 0` **and** any key falls in `[start, ripple_point)`, insert a key at
       `start − 1` valued `value_at(start − 1)`, carrying that segment's interpolation. Without it
       the segment from the last pre-cut key spans the join and re-shapes the pre-cut interior
       (rule 16's mechanism), changing the audio *before* the cut.
    2. Drop keys in `[start, ripple_point)`.
    3. Shift keys at `>= ripple_point` left by `duration`.
    **No key is inserted at `start`.** A key at exactly `ripple_point` shifts to exactly `start`,
    and it is that key — not an invented one carrying the value of material that no longer exists
    — that defines the post-cut side.
23. **`RippleInsertGap`, one step:** shift keys at `>= at` right by `duration`, on **the operation's
    own track set** — its target tracks plus every `sync_lock` track, which is not ripple delete's
    source-track set — and **insert nothing**. The predicate is exactly the clip predicate
    (`timeline_start >= at`, operation.rs:2450-2456), so a key exactly at `at` shifts. Nothing is
    inserted because the gap is empty on every shifted track by construction; saying that out loud
    is the reason the asymmetry with rule 22 is correct.
24. **No bus or master ripple.** A bus is fed by several tracks and a ripple's `sync_lock` set is
    per-track, so shifting a bus curve would desynchronise it from every track the ripple did not
    move. The "shift when the `sync_lock` set covers every feeding track" refinement is explicitly
    **not** adopted: it is a rule whose truth changes when a track is re-routed. Limit recorded;
    revisit in AU6.
25. **Speed leaves the envelope alone** and lets `E` clamp it. Keyframe frames are not scaled by
    `100/speed`: integer division makes the round trip lossy and can collapse two keys onto one
    frame, which `validate()` rejects. Video clip curves are not speed-scaled either
    (timeline.rs:209-211), and retimed clips contribute no audio at all
    (`timeline_audio_segments` skips `speed_percent != 100`, timeline.rs:255-263), so nothing
    audible can go wrong and returning to 100 % restores the surviving span exactly. **But a speed
    increase is destructive, and that is said out loud rather than implied away**: 200 % halves the
    project duration through `clip_effective_fps` (model.rs:1155-1160), so `E(0, new_duration)`
    **drops** every key past the new end, and returning to 100 % lengthens the clip while rule 15
    correctly inserts nothing — the dropped keys are gone and **only undo restores them**. Nothing
    is audible at the time only because retimed clips contribute no audio, which is what makes the
    loss silent. That is the cost of not scaling `at` by `100/speed`, it is the reason
    "speed-following envelopes" is a named deferral (§1.3), and it earns its own `### Changed`
    CHANGELOG line (§1.4).
26. **"Audio-aware trim behaviour", concretely — three clauses.**
    1. **Fades follow the trimmed edge.** `trim_clip` clamps `audio_fade_in_frames` and
       `audio_fade_out_frames` against the new **project** duration `doc.clip_duration(clip)?`,
       reducing the fade-out first and then the fade-in if the sum still exceeds it. It sits beside
       the title clamp (operation.rs:1737-1744) and is deliberately **not** the same expression: the
       title clamp's local `duration` is `new_source.end − new_source.start`, in **source** frames,
       which the §2.4 preamble says is not `new_duration`. The two coincide for Title and Freeze
       clips, whose source fps equals `doc.fps`, and diverge for a Media clip. `split_clip` gives
       the **left half the fade-in and the right half the fade-out**, each zeroed on the cut side,
       mirroring what it already does for title fades (:1672-1682) — today both halves keep both
       fades verbatim, and a split of a fully-faded clip therefore fails with `AudioFadesTooLong`.
    2. **The envelope's value at the new edge is preserved** (rule 13 step 1).
    3. **Nothing fails.** Rule 20.
    This is the second reject → clamp change and it gets its own CHANGELOG line (§1.4). Both
    existing pins of `AudioFadesTooLong` (contracts.rs:3617 via `SetClipAudio`, :3688 via
    hand-edited document validation) are on *writing* a bad value, not on trimming, so both survive.

### 2.5 The two operations and the `set_track_mix` merge (D12, C1, C9, C10, C15, C21)

```rust
/// AU4 §2.5: replace or clear one clip's gain envelope, in clip-local frames.
/// `null` clears it; the field is required, so an omitted `curve` is an error
/// and never a silent clear.
SetClipGainEnvelope {
    clip: ClipId,
    #[schemars(required)]
    curve: Option<AutomationCurve>,
},
/// AU4 §2.5: replace or clear one track's gain or pan automation, in project frames.
SetTrackAutomation {
    track: TrackId,
    #[schemars(extend("enum" = ["gain_tenth_db", "pan_percent"]))]
    parameter: String,
    #[schemars(required)]
    curve: Option<AutomationCurve>,
},
```

```rust
// core model.rs, beside TRACK_MIX_GAIN_MIN/MAX
/// AU4 §2.5: the closed vocabulary of `SetTrackAutomation.parameter`, spelled
/// exactly as the `TrackMix` fields the curves park on.
pub const TRACK_AUTOMATION_PARAMETERS: [&str; 2] = ["gain_tenth_db", "pan_percent"];
/// AU4 §5.1: the bottom of the timeline rubber band's display axis, in tenth-dB.
/// A display bound only — no validator reads it, and a key below it is legal and
/// paints against the band's edge.
pub const ENVELOPE_DISPLAY_MIN_TENTH_DB: i32 = -400;
```

27. **Normative.** Both variants are idempotent whole-curve replaces, so snapshot undo needs no
    per-variant code and setting a curve equal to the stored one produces an identical document.
28. **`curve: Option<AutomationCurve>` with `#[schemars(required)]`, and why.** serde's derive
    already rejects an omitted field for an attribute-free `Option<T>`; schemars 1.2.2 does not
    publish it as required — `schemars_derive/src/schema_exprs.rs:730-740` computes `is_optional`
    as `contract().is_deserialize() && <ty>::_schemars_private_is_option()` when the field carries
    no `default`, no `skip_serializing_if` and no `#[schemars(required)]`. Without the attribute the
    tool schema would tell the agent `curve` is optional and serde would then reject the call at
    runtime with a bare `missing field \`curve\``: a schema/deserializer disagreement on a mutating
    tool, which costs an agent a turn and is invisible to any test that always passes the field.
    This is the **first `#[schemars(required)]` in the workspace** (`grep` finds none), which is why
    it is written down here. Both halves are pinned: `"curve"` appears in each variant's schema
    `required` list, and `serde_json::from_value` errors on omission while `"curve": null` clears.
29. **Deviation from the two-variant precedent, named and defended.** `SetEffectKeyframes { …,
    curve: AutomationCurve }` plus a separate `ClearEffectKeyframes` (operation.rs:274-285) is the
    shape AU4 copies its *whole-curve* semantics from, but not its *arity*. One `Option`-bearing
    tool per owner instead of two saves two generated tools at ≈22 kB of schema each — about
    **44 kB** of `input_schema_bytes` — at the cost of the omission-versus-null subtlety rule 28
    closes. 52 → 54 generated tools rather than 52 → 56.
30. **`parameter: String` with an inlined enum.** `#[schemars(extend("enum" = [...]))]` puts the
    closed vocabulary in the field schema with **no `$defs` entry at all** (≈45 B per tool), so the
    agent sees the two legal values and N0.1's reuse rule is satisfied. The brief's "a new `$defs`
    type costs ~1,195 B on each of 50 tools" is AU2's measurement of *two whole struct/enum
    definitions plus a new field* (server.rs:21794-21802) and does not support a bare `String`;
    the honest cost of a two-variant enum would have been ~100–150 B of `$defs` plus its references.
    `OpError::UnknownTrackAutomationParameter` is kept regardless, for hand-edited documents and for
    an agent that ignores the enum, and one test pins `TRACK_AUTOMATION_PARAMETERS` against the
    schema's `enum` list **and** against the `render_*` keys (§4.1) so the three cannot drift.
31. **`SetClipGainEnvelope` rejects non-media clips** with the existing `TitleClipHasNoAudio` /
    `FreezeClipHasNoAudio` (`set_clip_audio`, operation.rs:3179-3187 — the two arms at
    :3184-3185), matching it arm for arm.
    `SetClipGainEnvelope { curve: Some(AutomationCurve { keyframes: vec![] }) }` is rejected as
    `InvalidClipGainEnvelope { reason: "automation curve must contain at least one keyframe" }`
    through `AutomationCurve::validate`'s `Empty` — written down here beside "`None` clears" so the
    two are never conflated.
32. **`set_track_mix` MERGES.** Normative, and the highest-value line in this section.
    `set_track_mix` (operation.rs:1194-1218) constructs a fresh `TrackMix`; `SetTrackMix` carries
    five scalars and no curve. It must therefore read the stored entry's `gain_curve` and
    `pan_curve` into the fresh entry **before** `validate_track_mix_values` and **before**
    `is_neutral()`. Without the merge, one nudge of a fader on an automated track silently drops
    both curves, and the `retain` arm additionally deletes the entry whenever the five scalars are
    neutral. It is undoable but invisible, and it is a data-loss bug on the flagship gesture.
32a. **`set_track_automation` shares `set_track_mix`'s entry lifecycle.** Normative. It reads the
    stored entry or `TrackMix::neutral(track)` (model.rs:758-764), writes the named curve,
    validates, and then applies the **same** elision `set_track_mix` does: an entry that is
    `is_neutral()` under rule 11 — five neutral scalars *and* both curves `None` — is removed from
    `audio_mix.tracks` by the `retain` arm (operation.rs:1213-1218); otherwise it is inserted or
    replaced and the vector re-sorted by `track`. A `SetTrackAutomation` on an un-mixed track
    therefore **pushes** a new entry, and a `SetTrackAutomation { curve: None }` that clears the
    last curve on a track whose five scalars are neutral **removes** it, so clearing the last curve
    leaves the document byte-identical to one that never carried it. Without this the wire would
    carry a `{"track":N}` entry a fresh document would not have, breaking N0's "defaults are omitted
    on the wire so goldens stay byte-unchanged" for any document that has ever carried and then
    cleared a curve — and `render_audio_mix`'s `is_neutral` gate (render.rs:437) would hide it from
    the compact rendering but not from the JSON. Pinned by A5.
33. **The strip's `Reset` keeps the curves.** `reset_row` (mixer_ui.rs:869-880) pushes
    `track_mix_operation(TrackMix::neutral(mix.track))`; under rule 32 that operation now clears the
    five scalars and preserves the two curves. `TrackMix::neutral` therefore keeps `gain_curve:
    None, pan_curve: None` and the merge is what preserves them, so the app needs no new operation.
    The tooltip becomes **"Return this track to unity gain, centre pan, unmuted and unsoloed.
    Automation is kept."** `reset_row`'s early return (mixer_ui.rs:869-871) is **replaced** — not
    doubled — by the scalar test `mix.gain_tenth_db == 0 && mix.pan_percent == 0 && !mix.mute &&
    !mix.solo`, the pre-AU4 meaning of `is_neutral`, so the button is drawn exactly when it can do
    something. Keeping `if mix.is_neutral() { return; }` beside it would be dead code: under
    rule 11 `is_neutral()` implies the scalar test, so the original branch can never fire as a
    distinct arm. The widened `is_neutral` is about **elision on the wire** and is deliberately not
    what this button asks. A curve-bearing track whose scalars are already neutral therefore draws
    no `Reset` at all, rather than drawing one whose click the app's no-op filter would swallow.
34. **Bus and master fader curves need no operation.** They ride inside `UpsertAudioBus` and
    `SetAudioMaster`, which the mixer's `MixerChainEdits` fold already clones from the document
    (mixer_ui.rs:312-403). **Whole-owner-set semantics are accepted and named**: an agent that omits
    `gain_curve` from an `AudioBus` clears the bus's automation, exactly as omitting `effects`
    clears the chain today. It is made recoverable rather than safe: §4.1 spells `gain_curve=[…]`
    on the bus and master lines so the agent can read the curve before it writes, and a test
    round-trips a curve-bearing bus through `render_audio_mix` → `upsert_audio_bus`.
35. **Coalesce keys**, one per target, extending the ledger at inspector_ui.rs:244-269:
    `envelope_coalesce_key(clip) = "envelope:{clip}"` and
    `track_automation_coalesce_key(track, parameter) = "track_automation:{track}:{parameter}"`. Bus
    and master automation reuse the existing `audio_bus:{id}` and `audio_master` keys because the
    curve rides inside the whole-chain set. One rubber-band drag is one undo entry via
    `DoBatchCoalesced` with `{key}#{gesture}`. Both spellings and
    `ENVELOPE_DISPLAY_MIN_TENTH_DB` land in **Part A** even though nothing reads them until Part B,
    so Part B carries no core change.

### 2.6 Validation and the new `OpError` variants (§6 of the brief)

Nine new variants, message shapes following the AU1/AU2/AU3 table style and the existing siblings
quoted verbatim beside them:

```rust
#[error("clip {clip} gain envelope is invalid: {reason}")]
InvalidClipGainEnvelope { clip: ClipId, reason: String },
#[error("track {track} automation for {parameter:?} is invalid: {reason}")]
InvalidTrackAutomation { track: TrackId, parameter: String, reason: String },
#[error("clip {clip} gain envelope value {value} is outside the inclusive range -600..=120")]
ClipGainEnvelopeOutOfRange { clip: ClipId, value: i64 },
#[error(
    "automation keyframe {at} for clip {clip}'s gain envelope is outside its local range 0..{duration}"
)]
ClipGainEnvelopeKeyframeOutsideClip { clip: ClipId, at: TimeCode, duration: TimeCode },
#[error("track {track} automation value {value} for {parameter:?} is outside its inclusive range")]
TrackAutomationOutOfRange { track: TrackId, parameter: String, value: i64 },
#[error(
    "automation keyframe {at} for track {track} parameter {parameter:?} is outside project range 0..{duration}"
)]
TrackAutomationKeyframeOutsideProject {
    track: TrackId, parameter: String, at: TimeCode, duration: TimeCode,
},
#[error("unknown track automation parameter {parameter:?}; expected gain_tenth_db or pan_percent")]
UnknownTrackAutomationParameter { parameter: String },
#[error(
    "automation keyframe {at} for audio bus {bus}'s fader is outside project range 0..{duration}"
)]
AudioBusGainKeyframeOutsideProject { bus: AudioBusId, at: TimeCode, duration: TimeCode },
#[error("automation keyframe {at} for the audio master's fader is outside project range 0..{duration}")]
AudioMasterGainKeyframeOutsideProject { at: TimeCode, duration: TimeCode },
```

36. `InvalidClipGainEnvelope` and `InvalidTrackAutomation` wrap `AutomationCurveError` by
    `error.to_string()`, exactly as `InvalidEffectAutomation` does (operation.rs:3388-3392), so the
    three structural reasons ("automation curve must contain at least one keyframe", "automation
    keyframe positions must be non-negative", "automation keyframes must be strictly ordered by
    frame") reach the agent unchanged.
37. **Normative.** Every rule below runs **in the operation and as a document invariant**
    (`validate_document`, operation.rs:3783), the AU1/AU2 rule that makes one check cover the
    operation, the before/after clone (`ApplyOp for Operation`, :420-427), `Document::validate`, the
    app's `load_document` and `Core::spawn`. Per curve, in this order:
    1. `AutomationCurve::validate()` — non-empty, non-negative, strictly increasing, duplicates
       rejected.
    2. Every `value` inside the owner's declared range (`-600..=120` for the four gain owners,
       `-100..=100` for `pan_curve`).
    3. The bound: `at < clip_duration` for `Clip.audio_gain_curve`, and
       `at < duration || (duration <= 0 && at == 0)` against `document.duration` for the four
       project-frame owners. The relaxed form is normative and applies **equally** to the existing
       `Audio{Bus,Master}KeyframeOutsideProject` check on `Effect.keyframes`
       (operation.rs:4188-4204), which today has no `duration == 0` guard and therefore rejects
       every key — including one at frame 0 — on an empty timeline. A one-key constant curve on an
       empty timeline is a legal document (rule 17).
38. **AU2 §2.2's two rules are untouched and do not apply here.** None of these five parameters is a
    switch, so the hold-only rule (`is_hold_only_parameter`, operation.rs:3446-3477) has nothing to
    say about them and all five interpolations are legal; none is latency-bearing, so
    `is_static_audio_parameter` (effect.rs:3102-3110) has nothing to say either. AU4 adds no new
    entry to either predicate. `is_hold_only_parameter` becomes `pub` and is re-exported from
    `lib.rs` so §5.4's combo can filter with it — its first reader outside core.
39. `SetTrackAutomation` with a `parameter` outside `TRACK_AUTOMATION_PARAMETERS` is
    `UnknownTrackAutomationParameter`, raised **before** the curve is validated, so an agent typo
    gets the vocabulary back rather than a structural complaint.
40. **Idempotency and the no-op filter.** Setting a curve equal to the stored one is accepted and
    produces an identical document; the app's own no-op filter (`record_mix_edit`
    mixer_ui.rs:996-1015, `handle_matte_pointer` preview_ui.rs:1085-1141) drops it before it
    becomes an undo entry. The app never pushes an operation that equals the document (N0).

### 2.7 Part A contract tests

New `crates/kinewright-core/tests/au4_core.rs` in `au3_core.rs`'s style (claim-sentence names,
`/// AU4 §7 item` doc lines, in-file builders); `tests/contracts.rs` gains the wire-shape pins and
both new variants in `document_and_every_operation_variant_round_trip_through_json` (:178, entries
placed after `SetClipAudio` and after `SetTrackMix` respectively). Items A1–A8.

## 3. Part A media

### 3.1 `automation_step` — the one place automation becomes a float (D4, C3, C7, C20)

```rust
// core automation.rs, beside `value_at`
/// AU4 §3.1: the value stepped discontinuously at the first sample of `at`.
///
/// `Some` when there is a key exactly at `at` whose *preceding* segment is
/// `Hold`. Integer only; the caller turns it into the forward declick.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HoldStep { pub previous_value: i64, pub next_key: Option<TimeCode> }

impl AutomationCurve {
    /// AU4 §3.1: whether the value is flat from `at` to `at + 1` — outside the
    /// keyed interval, or inside a `Hold` segment.
    #[must_use] pub fn holds_at(&self, at: TimeCode) -> bool;
    /// AU4 §3.1: see [`HoldStep`].
    #[must_use] pub fn hold_step_at(&self, at: TimeCode) -> Option<HoldStep>;
}
```

```rust
// media audio.rs, beside track_mix_ramp_frames
/// AU4 §3.1: the forward declick applied to a `Hold` step, in milliseconds.
///
/// Deliberately equal to AU1's `TRACK_MIX_RAMP_MILLISECONDS` and deliberately a
/// **separate** constant: AU1's ramp is a playback-only transient that export
/// never runs (AU1 §3.4), while this one is derived from the document and is
/// therefore present in export, in playback, and in every measurement.
pub(crate) const AUTOMATION_HOLD_DECLICK_MILLISECONDS: u32 = 5;
/// 240 sample frames at 48 kHz.
fn automation_hold_declick_frames(sample_rate: u32) -> u64;

/// AU4 §3.1: the two integer frame anchors bracketing one absolute project
/// sample, the weight between them, and the `Hold` declick term.
///
/// Integer in, three floats out; the same call in both mix paths.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct AutomationStep {
    pub v0: i64,
    pub v1: i64,
    pub t: f32,
    /// `Some((held, w))` asks the consumer for one more lerp, from the amplitude
    /// of `held` toward the value it just computed, at weight `w` in `0.0..=1.0`.
    pub declick: Option<(i64, f32)>,
}

pub(crate) fn automation_step(
    curve: Option<&AutomationCurve>,
    static_value: i64,
    origin: TimeCode,
    project_sample: u64,
    sample_rate: u32,
    project_fps: Rational,
) -> AutomationStep;
```

41. **Normative, step by step.**
    1. `f = samples_to_frame(project_sample, sample_rate, project_fps).0 − origin.0`. **If
       `f < 0`, set `f = 0` and `t = 0.0` before anything else**: `frame_to_samples` returns 0 for a
       non-positive frame (clock.rs:16-28), so a negative local frame must be handled *before* the
       call and never by it.
    2. `s0 = frame_to_samples(TimeCode(f + origin.0), …)`, `s1 = frame_to_samples(TimeCode(f +
       origin.0 + 1), …)`. Both are exact u128 floors (clock.rs:7-28).
    3. `v0 = curve.and_then(|c| c.value_at(TimeCode(f))).unwrap_or(static_value)`.
    4. `v1 = if curve.is_none_or(|c| c.holds_at(TimeCode(f))) { v0 } else {
       curve.value_at(TimeCode(f + 1)).unwrap_or(static_value) }` — **segment-aware**, which is what
       makes a `Hold` segment flat.
    5. `t = if s1 <= s0 + 1 { 0.0 } else { ((project_sample − s0) as f32) / ((s1 − s0) as f32) }`,
       clamped into `0.0..=1.0`.
    6. **Forward declick.** If `curve.hold_step_at(TimeCode(f))` is `Some(HoldStep { previous_value,
       next_key })` and `project_sample − s0 < D`, then `declick = Some((previous_value,
       (project_sample − s0 + 1) as f32 / D as f32))`, where
       `D = max(1, min(automation_hold_declick_frames(rate), s1 − s0, segment_span))` and
       `segment_span = frame_to_samples(next_key + origin) − s0` when `next_key` is `Some`, else
       `s1 − s0`. Otherwise `declick = None`. **`next_key` is the frame of the key *following* the
       one at `at`**, in the owner's own time base, and `None` when the key at `at` is the last.
       Both extra `min` terms are **defensive**: keys are strictly increasing
       (`AutomationCurve::validate`, automation.rs:65-72), so `next_key >= at + 1` and
       `segment_span >= s1 − s0` always; and `s1 − s0` is **1 600** sample frames at 30 fps /
       48 kHz, so `D == automation_hold_declick_frames(rate)` at every supported rate. They are
       written down so the formula stays correct at a rate where a project frame is shorter than
       the declick — above 200 fps at 48 kHz — and not because either binds today.
42. **Consumers, normative.** Gain is
    `let a0 = db_gain(v0); let a1 = db_gain(v1); let target = a0 + (a1 − a0) * t;` then, when
    `declick` is `Some((held, w))`, `let h = db_gain(held); if w >= 1.0 { target } else { h +
    (target − h) * w }`. **The `w >= 1.0` arm is normative, not an optimisation**: `h + (target −
    h)` is not `target`. For a large step — `h = 1.0`, `target = 1.0e-3`, the −60 dB duck of §6.1 —
    `fl(target − h)` carries an error up to `ulp(0.999)/2 ≈ 6.0e-8` and adding `h` back
    re-introduces all of it, putting the result ≈ 500 ulps from `target` (`ulp(1.0e-3) ≈
    1.2e-10`). Catastrophic cancellation is unavoidable whenever `h ≫ target`, and the value at the
    end of a declick is **defined** to be the automated value exactly. Pan is the same
    two-step composition applied per channel to `pan_channel_ratios(law, v0)` and
    `pan_channel_ratios(law, v1)`. **Both lerps are written `a + (b − a) * t`, never
    `(1 − t) * a + t * b`.** `a + (a − a) * t == a` bit-exactly for every finite `t`, which is the
    property that makes a constant curve cost nothing; the two-rounding form does not have it, and
    it is the form an implementer reaches for first.
43. **Pinned at `t = 0`.** `automation_step` at the first sample of any project frame returns
    `t == 0.0`, and `a0 + (a1 − a0) * 0.0` is bit-identical to `a0`, so the automated path at every
    frame's first sample equals `db_gain(value_at(f))` exactly. Asserted with `to_bits()`. **The
    multiply order is pinned with it**: `track_stage_parameters` returns `[gain * pan[0], gain *
    pan[1]]` (audio.rs:1566-1567), so the automated arm composes `automated_gain *
    automated_pan[ch]` in that order — never `pan * gain`, never a fused three-way product. Float
    multiplication is not associative, and A13's `to_bits()` neutrality assert depends on this
    clause as much as on rule 42's lerp form.
44. **The `Hold` rule, in one sentence and one pinned example.** *A `Hold` segment is flat; the
    value changes at the next key's first sample and is declicked forward over
    `min(5 ms, segment length)` after it.* At 30 fps and 48 kHz a project frame is **1 600** sample
    frames and the declick is **240**. For keys `{10: A, Hold}`, `{100: B}`: every sample of frames
    10…99 reads `A` exactly; the change begins at project sample
    `frame_to_samples(100) = 160_000`; `B` is reached at sample **160 239** (`w == 1.0`) and is
    exact from **160 240** on. Both indices are asserted and printed as
    `AU4_HOLD_STEP boundary=160000 reached=160239 exact=160240`.
45. **There is no seam, and that is asserted.** The last declick sample (`project_sample − s0 + 1
    == D`) returns `target` **bit-identically** to the first undeclicked sample's expression, by
    `to_bits()`; the test prints `AU4_HOLD_SEAM reached=… bits_equal=true`. Every earlier declick
    sample is strictly between `h` and `target` and is monotone in `project_sample`, and **that** is
    the property asserted in place of a tolerance. A tolerance was the revision-1 rule and it could
    not have gone green: `f32::EPSILON * target` is `1.19e-10` at `target = 1.0e-3` against a real
    error of ≈ 6e-8, three orders of magnitude out, on the flagship ducking gesture (rule 42).
46. **Memoisation.** Each **curve** — not each owner — carries its own per-frame memo of the two
    anchors, keyed on `(frame_index, curve_epoch)`, the AU2 per-frame cache idiom
    (audio.rs:818-828), so gain does not cost two `powf` per sample. Gain memoises two `f32`
    amplitudes; pan memoises two `[f32; 2]` ratio pairs (`pan_channel_ratios`, :1526-1547), which
    is why the memo cannot be keyed per *owner*: a track stage owns **two** curves and a
    frame-indexed per-owner memo collides between gain and pan. `curve_epoch` is a `u32` on
    `TrackStageRuntime`, `GainRamp` and `ClipAudioShaping`, bumped **unconditionally** by their
    `retarget`/rebuild, exactly as `AudioEffectNode::retarget` bumps `parameter_epoch`
    (audio.rs:812-816) — **those three types have no such field today**, which is why it is written
    down here; `parameter_epoch` belongs to `AudioEffectNode` and is reachable from none of them.
    Export never retargets, so the epoch is constant there and the memo changes nothing observable;
    live, it is what makes an edit audible on the **next sample frame** rather than up to 1 599
    samples later at the next project-frame boundary.
47. **Why per project frame and not per sample.** A sample time base would give every owner a
    different time base from the chain nodes beside it (audio.rs:2039-2048, :2100-2110), make a
    keyframe's `at` mean something different on a clip envelope than on the compressor threshold two
    stages later, multiply `value_at`'s O(n) scan by 48 000, and break AU2's pin
    `per_project_frame_evaluation_matches_a_per_sample_reference` (audio.rs:5464). A per-frame step
    with **no** ramp zippers at 33 ms; removing the zipper is why a ramp exists at all.
48. **The chord bound, stated.** Between two project frames the two constant-power ratio pairs are
    lerped, which is a chord across the arc. Over a one-frame pan step of `Δ` percent the deviation
    is bounded by `1 − cos(Δ·π/400)` and its exact worst case (the sagitta) is `1 − cos(Δ·π/800)`.
    At `Δ = 10` that is a bound of **3.083 × 10⁻³ (−0.027 dB)** against an exact **7.710 × 10⁻⁴
    (−0.0067 dB)**; at `Δ = 100`, the largest possible one-frame step, a bound of **0.293
    (−3.01 dB)** against an exact **0.0761 (−0.688 dB)**. The test sweeps every integer pan step and
    prints `AU4_PAN_CHORD delta=… bound=… measured=…`.
49. **The centre is not the identity under both laws.** `pan_channel_ratios(Balance, 0)` is
    `[1.0, 1.0]`; `pan_channel_ratios(ConstantPower, 0)` is `[FRAC_1_SQRT_2, FRAC_1_SQRT_2]`
    (−3.0103 dB, audio.rs:1533-1541, model.rs:600-601). The property that carries the weight is not
    "centre is unity" but **"lerping two identical ratio pairs is exact under both laws"**, which
    rule 42's lerp form gives. AU1's Balance-centre identity remains true and remains a
    Balance-only remark.

### 3.2 The clip envelope enters through `ClipAudioShaping` (D5, C16)

50. `ClipAudioShaping::new(clip, clip_duration, sample_rate, project_fps)` (audio.rs:447-478) gains
    `clip.audio_gain_curve.clone()` and stores `origin = clip.timeline_start`. The struct loses
    `Copy`; `gain_at` becomes `fn gain_at(&self, project_sample: u64) -> f32`.
51. `gain_at` gains **one factor**, evaluated first so the existing three ramps multiply into it
    unchanged: `envelope × constant_gain × fade_in × (1 − fade_out) × transition`. With no curve the
    factor is skipped entirely and the returned float is bit-identical to pre-AU4.
52. The envelope factor is `db_gain` of `automation_step(curve.as_ref(),
    clip.audio_gain_tenth_db as i64, origin, project_sample, rate, fps)` composed by rule 42. The
    static fallback is the clip's own `audio_gain_tenth_db`; when a curve exists it **replaces**
    that scalar, so `constant_gain` must be **1.0** in that case rather than multiplying twice —
    normative, and the single most likely implementation slip in this section.
53. **This is where exit-gate clause 1 is discharged structurally for clip envelopes.** There is one
    implementation and two call sites — export `mix_pass` (export.rs:1181-1193) and live
    `AudioMixSource::add_samples` (audio.rs:2242-2251) — and **both already pass the same absolute
    project sample index**. Identity is by construction, not by tolerance.
54. **Fades stay separate multiplicative ramps.** `audio_fade_in_frames`/`audio_fade_out_frames`
    keep their `fade_in + fade_out <= clip_duration` invariant (operation.rs:3631-3641) and keep
    composing multiplicatively. Folding fades into the envelope would change the wire of every
    existing document, break `SetClipAudio`, and lose the property that makes rule 26 cheap: a fade
    is a frame count that clamps to a new edge in one line; an envelope is not.

### 3.3 The automated track stage (D6, C4)

55. `track_stage_parameters(document, track, channels)` (audio.rs:1553-1568) keeps its signature and
    keeps returning the **parked** `(audible, gain, target)`. `TrackStageRuntime` gains
    `gain_curve: Option<AutomationCurve>` and `pan_curve: Option<AutomationCurve>`, cloned at `new`
    and at `retarget`, plus the parked `gain_tenth_db`/`pan_percent` and `audible` needed by rule 58.
56. `TrackStageRuntime::apply` gains the chunk's `start_sample: u64` and, when either curve is
    `Some`, walks the automated arm: for each source frame `i`, `n = start_sample + i`, compute the
    per-channel automated target from `automation_step` on each curve under rule 42, then apply the
    live ramp on top of it exactly as today. The neutral fast paths
    (`!audible → fill(0.0)` and `gain == 1.0 && target == [1.0, 1.0] → copy_from_slice`) are taken
    **only when both curves are `None`**, so a curve-free document stays bit-identical.
57. **Composition with AU1's 5 ms ramp is a strict layering.** The automated value is the *target*;
    while `ramp_index < ramp_frames` the applied value is
    `ramp_start + (target(n) − ramp_start) * ((i + 1) / ramp_frames)`, otherwise `target(n)`. The
    formula is already well defined when the target moves mid-ramp: `ramp_start` is fixed,
    `progress` reaches 1 at `ramp_frames`, and the applied value converges to the automated target
    exactly at the end of the ramp with no discontinuity. Because runtimes are built **settled**
    (`ramp_index == ramp_frames`, audio.rs:1585-1596), **export never ramps and is exactly the
    automated value**, and the ramp stays a playback-only transient after a live edit, as AU1 §3.4
    says.
58. **`current` means "the value last applied".** Normative. The automated arm writes the applied
    per-channel value back into `self.current` once per sample frame — it is already `[f32; 2]` — so
    `ramp_start = self.current` at the next `retarget` starts from a value that was audible one
    sample ago. Without this, `ramp_start` would be whatever the settled construction value was and
    the 5 ms ramp would jump to a stale value and ramp from there: it would manufacture precisely
    the click it exists to prevent, on the first live edit of any automated track.
59. **`retarget` ramps only when the document inputs changed.** Normative.
    `update_audio_mix` retargets **every** stage on every mix edit
    (audio.rs:1842-1850), so an unconditional `pending_ramp` would restart a 5 ms ramp on every
    other automated track and on both faders whenever one fader moved — a small, audible,
    edit-triggered modulation on channels the editor did not touch. `TrackStageRuntime::retarget`
    therefore compares the **document tuple** `(audible, gain_tenth_db, pan_percent, gain_curve,
    pan_curve, pan_law)` — every input `track_stage_parameters` (audio.rs:1553-1568) reads,
    **`pan_law` included**, because `SetPanLaw` is one of the five live variants (app.rs:1684-1693)
    and a law change moves the derived target
    (`pan_channel_ratios(document.audio_mix.pan_law, mix.pan_percent)`) without moving any
    per-track field. Omitting it would make switching Balance ↔ ConstantPower during playback a
    **no-op on every track** — `self.current` is never written on the unchanged arm and
    `channel_gain` reads it (audio.rs:1618-1624) — a live regression on an AU2 feature introduced
    by an AU4 rule. `TrackMix` is `PartialEq + Eq` under §2.1. The comparison sets
    `ramp_start = self.current`,
    `ramp_index = 0` only when it differs. The unchanged-target arm keeps `ramp_index =
    ramp_frames`. AU1's comment *"an exact identity test, not a tolerance question: an unchanged
    target must not restart the ramp"* stays on the function and now describes the document
    comparison.
60. **`skip_ramp` completes to the automated value on a curve-bearing stage.** Normative.
    `skip_ramp` (audio.rs:1656-1683) keeps its signature and its completion assignment, but the
    assignment becomes `self.current = automated_target(n_end)` — the automated value at the last
    output frame of the silent chunk — instead of the parked `self.target`, so rule 58's definition
    of `current` holds across gaps as well as across audible chunks. Leaving it as
    `self.current = self.target` (:1663-1667) would park `current` at the scalar during any silence,
    and a live mix edit made in a gap would then set `ramp_start` to the parked value and ramp the
    first audible chunk of the next clip from it — exactly the stale-value click rule 58 exists to
    prevent, moved from "the first edit on an automated track" to "an edit made during a gap". A
    silent chunk still writes **no samples**; only `current` moves.

### 3.4 The automated bus and master faders

61. `GainRamp` gains `curve: Option<AutomationCurve>`, the parked `gain_tenth_db`, and an
    `origin` of `TimeCode::ZERO` (these curves are project-frame keyed with no origin subtraction,
    which is exactly what the chain nodes beside them already do). `GainRamp::apply` gains
    `start_sample: u64` and takes the automated arm when `curve.is_some()`, under rules 42, 57
    and 58; `retarget` compares `(gain_tenth_db, curve)` under rule 59.
62. Placement is unchanged and normative: the **bus fader sits after the chain and before the pad**
    (audio.rs:2054-2057), the **master fader sits before the master effects** (audio.rs:2085-2088).
    A bus fader curve and that bus's own compressor curve, drawn at the same project frame,
    therefore act at the same time (§3.5).
63. `mix_chunk`'s three call sites already hold the absolute input sample index
    (`start_sample`), so no signature above `mix` changes: export.rs:1264, export.rs:1288,
    audio.rs:2473.

### 3.5 Latency: the automation key stays the input frame (D7, Q1)

64. **Normative.** `project_at` is derived from the **input** cursor and is not latency
    compensated, for chain nodes today (audio.rs:2039-2048, :2100-2110) and for every AU4 owner.
    Compensation happens on the buffer ends instead (export `mix_pass` :1301-1310, live
    `AudioMixer::open` :2364-2399), so the output *is* aligned to picture and the curve is not.
65. **The consequence, pinned rather than pretended away.** The automation key is the processor's
    **input** frame, and the head trim removes the whole graph latency from the master family
    (`drop_leading_samples(&mut mix, graph_latency_frames(…))`, export.rs:1119, :1300-1302; the
    comment beside it is decisive — *"Track stems are tapped pre-bus and carry nothing; bus stems
    are tapped after the bus stage's alignment pad; the master carries the whole graph latency"*).
    The resulting offset against the audible result is therefore **per owner**, and is **zero for
    the two owners this slice adds an editing surface to**:
    * `Clip.audio_gain_curve` and `TrackMix.gain_curve`/`pan_curve` — applied to the raw input
      chunk before any chain (audio.rs:1686-1746 via `mix_chunk`), so exactly **0** frames, with or
      without declared lookahead;
    * `AudioBus.gain_curve` — applied after the bus chain and **before** its alignment pad
      (audio.rs:2053-2059), so early by the bus chain's **own** accumulated node latency `ℓ_bus`,
      which is `0` without lookahead and at most `stage_latency_frames(CHAIN_LOOKAHEAD_MILLISECONDS,
      rate)` = **960** frames (20 ms) at 48 kHz (`CHAIN_LOOKAHEAD_MILLISECONDS = 20`,
      model.rs:645);
    * `AudioMaster.gain_curve` — applied to the summed, bus-padded mix (audio.rs:2088-2091), so
      early by `stage_latency_frames(L_bus, rate)`, again `0` without lookahead and at most **960**
      frames.

    The graph sum `stage_latency_frames(bus) + stage_latency_frames(master)` (audio.rs:502-517, the
    sum of two truncations and never a truncation of the sum) is the **trim**, not any owner's
    offset: material at input index `n` reaches the master output at `n + L_bus + L_master` and,
    after a trim of exactly that, lands at final position `n`. No owner is early by the sum. This
    amends R21's single figure (§0 R25).
66. **Why keeping it is right.** Compensating one owner and not another would make a bus fader
    curve and that bus's own compressor curve act at different times and leave AU2's chain nodes as
    the only uncompensated readers; and both paths stay identical by construction. The relative
    alignment of a clip envelope and a track ride is **exact by construction — both offsets are
    0** — which is the property the editor can feel, and it is exact because both are applied to
    the same raw input chunk, not because they share a compensation. Latency-compensated **bus and
    master** automation alignment is the named deferral (§1.3), and it is AU2's chain-node offset
    by another name.

### 3.6 Why `mix_pass` must keep starting at project sample 0

67. **Normative, recorded because it is load-bearing and invisible.** `mix_pass` builds each track
    buffer at the absolute offset `frame_to_samples(segment.project.start)` and computes
    `project_sample = start_frame + frame_offset` (export.rs:1156, :1174-1193); its chunk loop then
    always runs `start_frame` from **0** regardless of `range.start` and trims the head afterwards
    via `keep_from`/`drop_leading_samples` (:1246-1294, :1300-1320). `measure_mix_levels` and
    `measure_audio_qc` go through the same `mix_pass`, so a **windowed** measurement mixes from
    project sample 0 and keys every automated owner at the true project frame. A future
    optimisation that started the loop at `range.start` would break automation, playback/export
    parity and AU2's latency trim in one move. A comment says so at the loop.

### 3.7 Numeric discipline

68. **Integer everywhere above the sample**: keyframe `at` (`TimeCode`), keyframe `value` (`i64`,
    validated into the owner's range), the resolved value at every project frame (`value_at`, i128
    internally), and the frame index itself (`samples_to_frame`, exact u128 floor).
69. **f32 exactly twice per owner per sample** in the ordinary case — the weight `t` and the lerp
    between the two anchor amplitudes — and three times inside a declick window. Anchor amplitudes
    are f32 `db_gain` values (`10^(v/200)`, audio.rs:2155-2159, the expression AU1 §3.1 pins),
    memoised per project frame (rule 46). Amplitude-linear rather than dB-linear interpolation
    needs no defence beyond naming the three precedents it matches: `GainRamp::advance`
    (audio.rs:1471-1481), `TrackStageRuntime::advance_ramp` (:1628-1640) and `AudioGainRamp::gain_at`
    (:393-403) are all linear in amplitude over their span.
70. **Neutrality is bit-exact**, pinned with AU3's `to_bits()` idiom (audio.rs:7158-7224).
71. **Both paths.** Clip envelopes: one function, two call sites, the same integer (rule 53).
    Track, bus and master: one `AudioMixProcessor`, three `mix_chunk*` call sites, all keyed by an
    absolute input sample index (rule 63). The parity pin is two-tier: `assert_eq!` bit-identity on
    `automation_step` and on the processor against a per-sample reference, and AU1's existing
    end-to-end `assert_playback_matches_export` 1e-6 window compare (audio.rs:4155-4198) extended
    with an automation-bearing fixture — live and export decode independently, and that comparison
    was never bit-exact.
72. **No tolerance is OS-conditional** (N0, CC6 §6.3). Every measurement is printed beside its
    assert in the `AU2 A11` / `AU3_NORMALIZE` house style with a stable prefix.

### 3.8 `update_clip_shaping` and the live path (D13, C8, Q8)

73. **Normative.** `AudioMixSource` (audio.rs:2195-2212) gains `clip: ClipId`, set from
    `segment.clip` in `AudioMixer::open` (:2340-2358). `AudioMixer::update_clip_shaping(document)`
    rebuilds each source's `ClipAudioShaping` **in place** when the segment layout still matches —
    same source count, same `clip` ids in the same order, same `project_sample_start`/`_end` and
    `source_sample_start`/`_end` — the `chain_structure_matches` idiom (audio.rs:1273-1303). A
    mismatch returns `false` and the caller falls back to stop-and-re-cue, exactly as the chain path
    already does.
74. Neither `SetClipAudio` nor `SetClipGainEnvelope` can change a clip boundary, an asset, or the
    declared lookahead, so `can_retarget_audio_mix` (engine.rs:2036-2038) stays valid unchanged.
75. **Plumbing, all Part A, and it is ONE control.** Normative.
    `Control::UpdateAudioMix(Arc<Document>)` (engine.rs:285) becomes
    **`Control::UpdateAudio(LiveAudioChange, Arc<Document>)`**, carrying the kind beside the
    document, rather than gaining a second `UpdateClipShaping` variant. One control is the shape the
    `Both` case actually wants: the worker's single arm (engine.rs:1688) applies **mix then
    shaping from the same `Arc<Document>`**, so the two halves cannot interleave with a re-cue or
    with each other, which is the real guarantee — draining two controls in send order gives
    nothing observable that one does not. The arm beside the old `update_audio_mix`
    (:1763-1798) stores the document and calls the mixer, falling back to the same
    pause-and-re-cue branch on a structural mismatch;
    `Playback::update_clip_shaping(&self, doc: Arc<Document>) { self.set_document(doc); }` — a
    **defaulted** trait method, so no test double changes (core media.rs:1689-1725, N0) — sits
    beside the existing defaulted `update_audio_mix` (media.rs:1710-1712), and
    `can_retarget_audio_mix` (engine.rs:2036) is unchanged.
76. **The rebuild is not on the audio callback thread.** The mixer is driven by `fill_ring` from the
    worker and the cpal callback only drains the ring (`AudioStream::fill`, audio.rs:2656-2668;
    `build_stream` is :2671). Cloning a
    `Vec<Keyframe>` per source in `update_clip_shaping`, and per stage in `retarget`, is therefore
    not a realtime allocation and no lock-free machinery is owed. Said out loud so a reviewer does
    not demand a design the code does not need.
77. **This pays off an AU1 debt:** the clip-gain slider stops re-cueing. Today the running
    `AudioMixer` holds `ClipAudioShaping` snapshots built once at `open` (audio.rs:2350), so even an
    AU1 clip-gain drag forces a stop-and-re-cue.

### 3.9 Part A media evidence map

(a) rules 41–49 as `automation_step` unit tests. (b) rule 71's two-tier parity. (c) rule 65's
latency pin. (d) rules 51–53's shared-function identity. (e) rules 56–60's ramp composition.
(f) rule 67's comment and its regression test (a windowed `mix_levels` on an automated document
reads the same values as the full-range pass restricted to the same window).

## 4. Part A agent and app

### 4.1 `render_*` spellings (C10, C15)

All four are default-omitted, so every existing golden stays byte-unchanged until a curve exists.
The keyframe spelling is `render_effects`' own (render.rs:952-970): `at:value:Interp` with `Interp`
the `KeyframeInterpolation` `Debug` name (`Hold`, `Linear`, `EaseIn`, `EaseOut`, `EaseInOut`).

78. `render_clip_audio` (render.rs:409-421) gains `,envelope:[at:value:Interp,…]` after
    `fade_out:{n}f`, and its early return additionally requires `clip.audio_gain_curve.is_none()`.
79. `track_mix_fields` (:435-444) gains `,gain_curve:[…]` and `,pan_curve:[…]`, each present only
    when its own curve is; `mix.is_neutral()` under rule 11 already keeps a curve-bearing track's
    suffix on the line.
80. The bus line (:651-665) gains ` gain_curve=[…]` after `render_audio_gain`, and the
    `audio_master` line gains ` gain_curve=[…]` after `gain=`. `render_audio_mix`'s early return
    already keys off `is_neutral`, which rule 11 widened.
81. **Normative.** The two `SetTrackAutomation` parameter tokens and the two `TrackMix` render keys
    are pinned to each other by one test: `gain_tenth_db ↔ gain_curve`, `pan_percent ↔ pan_curve`,
    both derived from `TRACK_AUTOMATION_PARAMETERS` so a rename cannot drift them apart.
82. Pinned by the AU2 "omits the neutral" golden pair (render.rs:1227-1327): one golden with all
    five owners' curves present, one asserting the pre-AU4 rendering byte-for-byte when none is.

### 4.2 `get_audio_levels`' output shape

83. `measure_mix_levels` puts `document.track_mix(track.id)` straight into `TrackLevels.mix`
    (export.rs:1453-1462), which is serialized to the agent, so the two new `Option` fields land in
    `get_audio_levels`' structured content automatically. They are default-omitted, so the AU1
    golden (`au1_get_audio_levels_measures_the_real_mix`, mcp_server.rs:2665) stays byte-unchanged
    until a curve exists. A **golden pair** is added: the same report on a curve-bearing document
    carries both keys, and every leaf of the serialized report is still an integer, boolean, string
    or null (`assert_integer_leaves`, au3_core.rs:960-976).

### 4.3 Part A registry effect and the ledger procedure (C19)

84. **Counts.** +2 generated mutator tools (`set_clip_gain_envelope`, `set_track_automation`):
    generated 52 → **54**, inspectors 78 → 78, registry 130 → **132**. `INSPECTOR_TOOL_NAMES`
    (schema.rs:16) is unchanged in Part A. Manual work per new variant: one arm in
    `operation_tool_name`, prose in `describe_operation` beside the keyframe ops (:503-508), the
    two name pins (:877-878, :1358-1359), the `.idempotent(...)` list, and the
    `contracts.rs:228` round-trip entry. **`set_effect_keyframes` joins the `.idempotent(…)` list
    in the same edit** (schema.rs:387-400): it is the whole-curve-replace precedent rule 29 names,
    it is idempotent for the same reason the two new tools are, and its absence today is an
    inherited oversight rather than a decision. The annotation delta is measured with the rest under
    rule 87.
85. **Bytes are an estimate to be measured, not a prediction to be believed.** Reusing
    `AutomationCurve`/`Keyframe`/`KeyframeInterpolation` adds **no new `$defs` type** — they are
    already reachable from `Operation` through `SetEffectKeyframes` (operation.rs:274-280) — so the
    growth has two terms: the two new tools' own ≈22 kB inlined schemas each, and **field growth**
    on the shared `$defs` copy carried by each of the 50 pre-existing `Operation`-embedding tools,
    because `Clip`, `TrackMix`, `AudioBus` and `AudioMaster` are all reachable from `Operation` and
    each gains one or two properties. Order **+45 to +55 kB** of `input_schema_bytes`; the real
    figure is measured and the derivation comment is rewritten to a per-part byte split that sums
    exactly.
86. **The served quad cannot move**: the seven served tools (`get_timeline_state`,
    `search_capabilities`, `get_capability`, `invoke_capability`, `prepare_edit_plan`,
    `commit_edit_plan`, `discard_edit_plan`) embed no `Operation` schema at all. `(7, 5_660, 3_510,
    998)` is asserted byte-identical (server.rs:21864-21873).
87. **The ledger procedure, verbatim** (facts-agent §6). Change the surface; run
    `served_surface_is_small_and_keeps_the_internal_registry_discoverable` (server.rs:21740) and read
    the printed `ToolSurfaceMetrics` (:21750); update the four asserts at server.rs:21847-21873 —
    today `(serialized, served) = (1_425_658, 5_660)`, `input_schema_bytes = 1_295_459`,
    `description_bytes = 108_875`, served quad `(7, 5_660, 3_510, 998)`; extend the derivation
    comment (:21759-21846) with a per-part byte split that sums exactly; update
    `cc7_the_agent_surface_is_unchanged_by_this_slice` (mcp_server.rs:2482-2560) — counts, the name
    list (:2521-2530), the ordering assert, the served quad — and its running doc-comment ledger
    (:2463-2481); leave `INSPECTOR_TOOL_NAMES`' length and `server.rs:19478` alone in Part A; append
    one M36 row (M36-AGENT-RUNTIME-EFFICIENCY.md:95-116) plus the following prose paragraph.

### 4.4 `LiveAudioChange` in the app (C8, Q8)

```rust
/// AU4 §4.4: what a document change asks the running engine to do without re-cueing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LiveAudioChange { None, Mix, ClipShaping, Both }
```

88. **Normative.** `is_audio_mix_operation` (app.rs:1684-1693) keeps its five variants and gains
    **`SetTrackAutomation`**. A new `is_clip_audio_operation` covers **`SetClipAudio` and
    `SetClipGainEnvelope`**. `is_live_audio_mix_change` becomes
    `live_audio_change(journal_command, old, new) -> LiveAudioChange`: `None` for `Undo`, `Redo`,
    the initial snapshot, an empty batch, any batch containing an operation in neither set, or any
    change to the declared lookahead; otherwise `Mix`, `ClipShaping`, or `Both` according to which
    sets the batch's operations fall into.
89. The single consumer (app.rs:1179-1191) sends **one** `Control::UpdateAudio(kind, document)`
    (rule 75) and the worker branches on the kind: `Mix` → `update_audio_mix`; `ClipShaping` →
    `update_clip_shaping`; `Both` → **both, mix then shaping, from the same `Arc<Document>`**;
    `None` → the existing stop-and-re-cue. The order is fixed because it is readable, not because
    anything observable depends on it — the real guarantee is the **single document applied twice
    inside one control**, so the two arms cannot interleave with each other or with a re-cue.
    Widening the old bool and leaving the consumer alone would make a
    `SetClipAudio`/`SetClipGainEnvelope` batch take the live path and then apply **nothing** —
    silently worse than the re-cue it replaced — and a batch mixing one Mixer op with one inspector
    op in one coalesced gesture would apply only half.
90. `is_audio_mix_operation`'s doc comment keeps its claim honest: it now says "every variant here
    is an idempotent full set of one mix target"; the clip-audio predicate carries its own sentence.

### 4.5 Telemetry: derived, never published (D14)

91. **Normative.** "The fader follows the envelope while playing" is `value_at(frame)` at
    `Playback::position()` — the atomically published **audible** master position the app already
    reads every frame (audio.rs:2623-2634; `fed_position_samples` is a separate accessor). No new
    `Playback` field, no new atomic, no seek/pause/reset semantics, no `BaselineProofAnalysis`
    forwarding arm (nothing is added to `Analysis`), and the displayed integer is exactly the one
    the DSP used at that frame, up to rule 65's offset.
92. A `LiveLoudness`-style published value is rejected: that pattern exists because loudness is
    **measured**. An automation value is **derived**, and publishing it would duplicate document
    state in the engine. Cost is one O(n) `value_at` scan per displayed control per frame; the
    cursor is a named deferral (§1.3).
93. **Read mode only.** No write, touch or latch; no "record a fader move while playing".

### 4.6 QC gains no automation-aware finding

94. **Normative, and stated because it is a decision and not an omission.** `qa_document`
    (qa.rs) and `get_audio_qc` gain **no** automation-aware finding in AU4. An envelope that dips to
    the floor legitimately trips AU3's `audio_silent` and channel-balance checks on the range it
    covers, and that is the correct report: QC measures the signal, not the intent.
    `retimed_audio_muted` (qa.rs:250-262) keeps its meaning and now co-exists with an envelope that
    §2.4 rule 25 leaves on a retimed clip; the envelope is inaudible for the same reason the clip
    is. No new code, one sentence in this contract, and one test asserting that a curve-bearing
    document produces the same `qa_document` issue set as the same document without the curve.

---

# Part B — Automation editing and planners

---

## 5. Part B app surfaces

### 5.1 The timeline rubber band (C12, Q5)

**Geometry.** The envelope draws into **the same rect `paint_waveform` uses** —
`band.shrink2(vec2(space::HALF, space::ONE))` (timeline_ui.rs:1237-1249) — so the ride and the
waveform agree pixel for pixel. `band` starts at `rect.top() + 18.0` for a pure-audio asset and at
`rect.bottom() − rect.height() * 0.42` otherwise (:1231-1236). That rect is computed **once**, by
the pure helper of rule 95a, and every consumer takes it as an argument.

95. **Value axis, normative.** Linear in tenth-dB over `ENVELOPE_DISPLAY_MIN_TENTH_DB = -400 ..=
    TRACK_MIX_GAIN_MAX = 120` — −40 … +12 dB, a 520 tenth-dB span. Unity sits at
    `(120 − 0) / 520 = 23.1 %` from the top. A value outside the range clamps to the edge and paints
    a 2 px marker there rather than being hidden. Derived resolutions at the three heights that
    actually occur: a pure-audio clip on a 72 px track gives a **38 px** band (clip height
    `72 − 8 = 64`, minus the 18 px header, minus `2 × space::ONE`) = **13.7 tenth-dB per logical
    pixel (1.37 dB)**; an audio+video clip on a 72 px track gives **18.9 px** = **2.75 dB/px**; a
    pure-audio clip on the 44 px minimum track gives **10 px** = **5.2 dB/px**. The band is
    therefore the **coarse** gesture and the inspector list (§5.5) is the exact one; DESIGN.md says
    so in those words.
95a. **One rect, one pure helper.** Normative.

    ```rust
    /// AU4 §5.1: the envelope's paint, hit and interact rect. Pure; no `Ui`, no session state.
    fn envelope_band_rect(clip_rect: egui::Rect, kind: MediaKind) -> Option<egui::Rect>;
    ```

    It returns `band.shrink2(egui::vec2(space::HALF, space::ONE))` for `MediaKind::Audio |
    AudioVideo` and `None` otherwise, with `band` built exactly as timeline_ui.rs:1230-1235 builds
    it (`rect.top() + 18.0` for pure audio, `rect.bottom() − rect.height() * 0.42` otherwise). It is
    called **twice per clip per frame** — once at the interact site beside `body`/`left`/`right`
    (timeline_ui.rs:454-486) and once inside the paint path (:1245), which stops using its local
    expression — so the painted, hit-tested and interacted rect are **the same object**.
    `envelope_x_to_local_frame`, `envelope_local_frame_to_x`, `envelope_value_to_y`,
    `envelope_y_to_value`, `envelope_hit` and rule 98's `body_rect` intersection all take that rect.
    Without the helper the two sites disagree by `space::HALF = 2` px in x and `space::ONE = 4` px
    in y (theme.rs:43-44) — the drawn key and the grabbed key would not be the same key, which is
    precisely the failure R12 exists to prevent — and the shrunk rect is not even reachable at the
    allocation site, because `band` is built inside the painting path, which takes a `&Painter` and
    no `Ui`, some 750 lines after `body` is allocated. A clip whose asset is `MediaKind::Video`, and
    every Title and Freeze clip, has **no band, no paint and no interact**.
96. **x maps through the rect ratio, not through `pixels_per_frame`.** Normative, and the reason is
    concrete: `clip_width = (duration.0 as f32 * pixels_per_frame).max(24.0)` (timeline_ui.rs:439),
    so at any zoom where `duration × pixels_per_frame < 24` the painted rect is wider than the
    clip's true span and adjacent clips overlap in x. `paint_waveform` survives this by mapping
    columns through a clip-local ratio of `rect.width()` (:1637-1641); the envelope does the same:

    ```rust
    /// AU4 §5.1: pointer x to a clip-local frame, through the same rect ratio the
    /// waveform uses. Pure; no window, no session state.
    fn envelope_x_to_local_frame(rect: egui::Rect, duration: TimeCode, x: f32) -> TimeCode;
    // round_half_away_from_zero((x - rect.left()) / rect.width().max(1.0) * duration.0 as f32),
    // clamped to 0..=duration - 1
    fn envelope_local_frame_to_x(rect: egui::Rect, duration: TimeCode, at: TimeCode) -> f32;
    fn envelope_value_to_y(rect: egui::Rect, value: i32) -> f32;
    fn envelope_y_to_value(rect: egui::Rect, y: f32) -> i32;
    ```

    Snapping runs **on that mapping**: the snapped project frame from `nearest_snap` (:2097-2120) is
    converted to a clip-local frame by subtracting `timeline_start`, never by re-deriving x from
    `pixels_per_frame`.
97. **The 24 px rule.** Normative. `const ENVELOPE_MINIMUM_CLIP_WIDTH: f32 = 24.0` — the same 24 the
    clip-width floor uses. No envelope band is painted and **no envelope hit-testing runs** when
    `duration.0 as f32 * pixels_per_frame < ENVELOPE_MINIMUM_CLIP_WIDTH`, so the coarse gesture is
    never offered where it cannot land.
98. **Interact allocation order.** Normative. The envelope's `ui.interact` is allocated **after**
    `body`, `left` and `right` in the same frame, with id `("clip-envelope", clip.id.0)` and
    `Sense::click_and_drag()`, and its rect is **rule 95a's `envelope_band_rect`, intersected with
    `body_rect` horizontally** — `Rect::from_min_max(pos2(band.left().max(body_rect.left()),
    band.top()), pos2(band.right().min(body_rect.right()), band.bottom()))`, with `band` the
    helper's return and never a locally rebuilt one. egui 0.35.0 resolves overlapping interactive
    rects by *last registered wins* (`hit_test.rs:429-433` — the cargo-registry copy is the
    workspace version, Cargo.lock:1116-1118; the comment sits at :429 and the code at :430: "In
    case of a tie, take the last one = the one on top"), so an interact allocated **before** the
    body would lose every tie, and
    one allocated without the x restriction would **steal** the two `EDGE_HANDLE_WIDTH` trim
    handles. That citation lives in a comment at the allocation so nobody "fixes" the order later.
99. **No allocation unless the pointer is near the curve.** Normative. The interact is allocated
    only when a drag is in flight, or when a pure `envelope_hit(points, rect, pointer) ->
    Option<usize>` — mirroring `curve_editor_widget::nearest_point` (:318-326) and the matte
    overlay's `matte_hit_test` (imported at preview_ui.rs:15, called at :1157) — reports the
    pointer within `ENVELOPE_HIT_RADIUS = 9.0` of the
    polyline or a key. Body drags and the two 6 px trim handles are therefore untouched everywhere
    except within 9 px of the line. Envelope hover and drag fold into `clip_pointer_interaction`
    (:488, consumed at :826) so the canvas-click seek stays suppressed.
100. **A toolbar toggle `Envelopes`** (session state beside `pixels_per_frame`, **not** document
     state) hides the overlay and with it all hit-testing. A modifier key is rejected — Alt is
     already the snap bypass (:367) — and so is a mode toggle that disables clip drags: a modal
     timeline for one feature.
101. **Gestures, exactly `curve_editor_widget`'s three pure rules.** Click on the line inserts a key
     at the snapped clip-local frame with the curve's current value there; drag a key moves it, x
     constrained between its neighbours ±1 frame (`constrain_dragged_x` :108-124) and clamped to
     `0..=duration − 1`, y clamped to `-600..=120`; right-click, or `Delete`/`Backspace` while
     hovered, removes; **the last key cannot be removed** (clearing is the inspector's `Clear`,
     which sends `curve: None`). Every rule is a pure function over the key list, provable without a
     window.
102. **Paint.** Points at `ENVELOPE_POINT_RADIUS = 3.5`, polyline at `ENVELOPE_STROKE = 1.6`, in
     `ACCENT` when the clip is selected and `TEXT_PRIMARY_64` otherwise — accent stays reserved for
     selection and direct manipulation (DESIGN.md:280-282). The three literals are
     `curve_editor_widget`'s own (9.0 / 3.5 / 1.6), reused rather than re-invented.
103. Alt bypasses snapping through the existing frame-level flag; guides paint through the existing
     `nearest_snap` path. x always lands on a project frame.

### 5.2 The coalescing path the timeline lacks

104. **Normative.** `timeline()` (timeline_ui.rs:218-903) emits one batch per gesture at
     `drag_stopped` and never touches `InspectorEdits`, `push_live` or `edit_gesture`. It gains
     **one** `InspectorEdits`, used by the envelope only: `drag_started` → `begin_gesture`; every
     `dragged() || drag_stopped()` frame recomputes the whole key list, **skips the write when it
     equals the document**, else `extend_live(ops, envelope_coalesce_key(clip))`; one
     `submit_inspector_edits(edits)` at the end of the function. This is the CC5 matte overlay's
     shape verbatim (`handle_matte_pointer` preview_ui.rs:1085-1141, "One gesture is one undo
     entry").
105. Clip move, trim, marker and playhead drags keep their `drag_stopped`-only `pending_operations`
     batch unchanged; the two paths coexist and the envelope is the only user of the new one.
106. `is_live_drag` (`dragged() || drag_stopped()`, inspector_ui.rs:731-733) is imported rather than
     re-derived, so the release frame stays inside the gesture and is not filed as a second undo
     entry.

### 5.3 The Mixer strip: the `A` chip and the automated fader (C11, Q7)

107. **The `A` chip goes inline after the kind icon on `caption_row`** (mixer_ui.rs:883-912), as a
     one-glyph `theme::caps_label("A", color::TEXT_MUTED)` inside the existing `horizontal`. There
     is **no** optional inline slot today: `NO AUDIO` sits on a separate `ui.label` **below** the
     caption row, and the source says why — "the caption, the kind icon, and a tracked
     eight-character caps label are 90 px of content in a 72 px strip". A one-glyph label is not
     that label.
108. **Height cost is zero, and the worst case is measured rather than inherited.** The caption
     row's height is fixed from `interact_size.y = size::ICON_SM = 14` (mixer_ui.rs:886-890,
     theme.rs:75) and a `caps_label` is `type_size::MICRO` = 10 (theme.rs:118), shorter than the
     icon, so the chip adds **width, not height**. `a_track_strip_fits_the_mixer_dock`
     (mixer_ui.rs:1589-1624) — which today measures `plain`, `no audio` and `solo muted and
     non-neutral` — gains **the worst case: a `NO AUDIO`, `SILENCED`, off-neutral, automated
     track**, three extra rows *and* the chip, a combination no existing test builds. A `NO AUDIO`
     track that is *only* automated is **not** the worst case: under rule 33 an automated track
     with neutral scalars draws no `Reset`, so it is just the `NO AUDIO` case. The **only binding
     assert is `size.y <= 240.0`**, beside the existing width assert and a new one on
     `caps_label_width("A")` (:1709) against the 72 px strip; the `232` figure is **not** pinned —
     it is the doc comment of a *different* test (`a_bus_and_master_strip_fit_the_mixer_dock`,
     mixer_ui.rs:2424-2432) describing this one from the outside — and the measured height is
     recorded as a §0 erratum instead, in `measure_loudness_section`'s `const BUDGET` /
     `const MEASURED` style (mixer_pane_ui.rs:1527-1544). **Two fallbacks, in order, if the worst
     case exceeds 240 px:** suppress the `A` chip on a `NO AUDIO` strip, and then tint the fader
     rail instead of drawing a glyph at all. The tinted rail alone would not help — the overflow
     would come from rule 11's widened `is_neutral` making `Reset` reachable beside two extra
     labels, not from the glyph — and neither fallback is a re-layout of the strip, which has 8 px
     free.
109. **The automated fader: disable the rail, keep the readout editable.** Normative. While the
     track carries a `gain_curve`, the fader's **rail** is disabled and displays the automated value
     at the audible position (rule 91); the **numeric readout stays editable** and a typed value
     writes the **parked scalar** under CC5's rule. Same treatment for `pan_control` and
     `pan_curve`. The hover text becomes **"Automation drives this fader. The number is the parked
     value and typing sets it; clear the automation in the chain pane to ride the fader directly."**
110. **Why the split, in the two DESIGN.md sentences it earns.** One rule is CC5's, unchanged:
     *editing a keyframed control writes its static value* (`MATTE_KEYFRAME_NOTE`,
     inspector_ui.rs:2381-2382). The other is new and is an affordance rule: *a control you can
     **drag** never lies about what you hear.* On a fader the editor believes they are riding what
     they hear, and CC5's write-the-static-value rule is invisible there; in a numeric field it is
     not.
111. `record_mix_edit`'s no-op filter and coalesce key are unchanged; the readout's write goes
     through `track_mix_operation`, whose merge (rule 32) preserves the curves.

### 5.4 The Mixer chain pane's `AUTOMATION` section

112. **Normative.** A new `AUTOMATION` section in the 400 px chain pane, **below** `LOUDNESS` on the
     master pane and at the top of a bus pane, editing **one parameter at a time**. This discharges
     AU2's deferral *"Automation editing UI for bus and master parameters"* by surface.
113. The parameter is chosen from a compact combo listing **`Fader`** plus every parameter of the
     chain's nodes that is neither hold-only nor static — `is_hold_only_parameter` (now `pub`, rule
     38) and `is_static_audio_parameter` gain their **first app readers**, so the app stops guessing
     automatability from `mixer_unit()`'s name match.
114. The editor is a keyframe list on a **generalised** `keyframe_row`. Normative:
     `keyframe_row(ui: &mut egui::Ui, clip: ClipId, …)` (inspector_ui.rs:3864-3882) is clip-scoped
     and lives in the inspector, while this section edits an `AudioBus`/`AudioMaster` fader that has
     no clip, so it becomes `pub(crate) fn keyframe_row(ui: &mut egui::Ui, key: &str, curve:
     &AutomationCurve, index: usize) -> KeyframeRowAction`, returning a **pure action** the caller
     turns into its own operation, re-exported from `inspector_ui.rs`. The inspector's existing
     caller is rewritten onto it **in the same commit**; "reuse the existing `keyframe_row`" is
     otherwise a no-op reuse of something that is not available. The rows stay
     `{frame} {value} {interpolation}` with `+ Key at playhead` and `Clear`, scrolled inside the
     pane. `Fader` writes `AudioBus.gain_curve` / `AudioMaster.gain_curve` through
     `MixerChainEdits`' existing fold (one `UpsertAudioBus`/`SetAudioMaster` per chain per frame);
     a node parameter writes `Effect.keyframes` the same way.
115. `mixer_pane_ui::parameter_value` (:766-772) starts resolving through
     `Effect::integer_parameter_at` at the **audible** frame instead of `static_integer_parameter`,
     so a keyframed card stops displaying its neutral. The EQ well's `TimeCode::ZERO` sample
     (:1122) moves to the same frame.
116. **Height is measured and pinned** the way `measure_loudness_section` is (`const BUDGET: f32 =
     90.0; const MEASURED: f32 = 86.0`, mixer_pane_ui.rs:1528-1529, asserted at ±1 px): a new
     `measure_automation_section` with `const BUDGET: f32 = 120.0` and a `const MEASURED` filled in
     from the first run, asserted at ±1 px, taken with an empty curve and with a scrolled ten-key
     curve, the list scrolled to at most four visible rows. The measured figure is recorded as a §0
     erratum in AU3 §0 E38's manner — the sentence is the contract and the budget is the constraint
     it is checked against. **The pane is asserted as well as the section.** On the master pane a
     120 px `AUTOMATION` section sits below a `LOUDNESS` section that measures **86 px against its
     own 90 px budget** (not 86 of the pane's 400), so the two together are a pane-level claim: B9
     carries a numbered `BUDGET` assert on a master pane that carries **both** sections, measured
     with an empty curve and with a scrolled ten-key curve, and the pane still fits a 260 px dock
     and scrolls.
117. Full automation lanes on the timeline are **rejected here and deferred** (§1.3): the row asks
     for editing "in the mixer" and this is the smallest surface that is it.

### 5.5 The inspector `ENVELOPE` block

118. The `Audio` section (inspector_ui.rs:1010-1095) gains an `ENVELOPE` block under the gain
     slider: the same `keyframe_row` list in **tenth-dB**, `+ Key at playhead`, `Clear` (which sends
     `SetClipGainEnvelope { curve: None }`), and a `KEYFRAMED`-style note. This is the precise,
     typeable surface DESIGN.md points the rubber band at.
119. The clip-gain slider follows the existing rule (`parameter_is_keyframed` :3887-3889, the
     `KEYFRAMED` badge and its hover at :2180-2185): it shows and writes the parked scalar. It is
     **not** disabled — rule 110's affordance rule is about controls whose position claims to be
     what you hear, and the inspector's slider sits directly above the list that owns the ride.
120. Every edit here is discrete except a drag on an existing key's value, which uses
     `envelope_coalesce_key(clip)` so it shares one undo entry with the rubber band's.

### 5.6 DESIGN.md (Part B)

121. `### Timeline` gains a paragraph naming: the envelope band and the fact that it is
     `band.shrink2(vec2(2, 4))`, the same rect the waveform uses; the 9.0 hit radius, the 3.5 point
     radius and the 1.6 stroke; the `Envelopes` toolbar toggle; that Alt bypasses snapping on
     envelope keys as it does everywhere; the −40 … +12 dB range; and the sentence **"the band is
     the coarse gesture and the inspector's keyframe list is the exact one"**.
122. `### Mixer` gains: the `AUTOMATION` section and its re-measured height; the inline `A` chip
     and the re-measured strip height; and rule 110's **two** sentences, one for CC5's rule and one
     for the drag affordance rule.
123. Both sections are pinned by `include_str!` tests. `the_design_note_states_the_new_mixer_rules`
     (mixer_ui.rs:3751-3825) gains the Mixer phrases; the **timeline gets its first**
     `include_str!` test, in `timeline_ui.rs`, slicing DESIGN.md between `"### Timeline"` and the
     next `"\n### "`.

## 6. Part B agent

### 6.1 `plan_audio_ducking` (C13)

The flagship. Evidence no audio planner reads today: a dialogue track's `TimelineSilenceSpan`s
(media.rs:1487-1496) and diarized `TranscriptWord`s (:1343-1351), both already in **project frames**.

```rust
struct PlanAudioDuckingArgs {
    music_track: TrackId,
    dialogue_tracks: Vec<TrackId>,
    #[serde(default)] depth_tenth_db: Option<i32>,           // default -120
    #[serde(default)] attack_milliseconds: Option<u32>,      // default 150
    #[serde(default)] release_milliseconds: Option<u32>,     // default 400
    #[serde(default)] hold_milliseconds: Option<u32>,        // default 200
    #[serde(default)] range: Option<TranscriptRangeArgs>,
    /// Replace an existing gain curve on the music track instead of refusing.
    #[serde(default)] replace: bool,
}
```

124. **The curve is keyed RELATIVE to the stored scalar.** Normative. `u = mix.gain_tenth_db` (the
     parked value) is the un-ducked value and `d = clamp(u + depth_tenth_db, -600, 120)` is the
     ducked one. Because a curve **replaces** its scalar (rule 9), a planner that keyed around
     `0 / depth` on a music track the editor had already pulled to −60 dB would raise the un-ducked
     music by 60 dB. The planner reads `document.track_mix(music_track)` and says so in its text.
125. **An existing `gain_curve` is REFUSED unless `replace: true`**, with the named reason
     `"track {t} already carries a gain curve; clear it or pass replace: true"`. A silent whole-curve
     replace of an editor's ride is not recoverable from structured content.
126. **Window construction, normative.** Merge every dialogue track's spoken spans into one span
     list in project frames, extend each span's end by `hold_frames`, then merge overlapping spans.
     For each merged span `[a, b)` emit four keys, all `Linear`:
     `(a − attack_frames, u)`, `(a, d)`, `(b, d)`, `(b + release_frames, u)`.
     `attack_frames = ceil(attack_ms × fps / 1000)` and likewise for release and hold — at 30 fps
     the defaults are **5**, **12** and **6** frames. Every `at` is clamped into
     `0..=document.duration − 1`; keys are deduped last-wins and the result is validated by
     `AutomationCurve::validate` before the plan is prepared.
     **`range`, normative.** When present (`TranscriptRangeArgs { start, end }`,
     server.rs:9829-9832) it restricts **which dialogue spans are considered** — a span is kept when
     it intersects `[range.start, range.end)` — and clamps the emitted curve to that window; every
     emitted `at` still clamps into `0..=document.duration − 1`. It does **not** restrict the
     measurement, which always uses the longest qualifying flat windows inside the emitted curve
     (rule 128). An argument with no stated semantics on a mutating planner is worse than no
     argument, which is why this clause exists rather than the field being dropped; it is echoed in
     structured content (rule 129) and tested (B11).
127. It emits **one** `SetTrackAutomation { track: music_track, parameter: "gain_tenth_db", curve }`
     inside one `prepare_operations` plan (server.rs:7728-7740) — track automation and not a clip
     envelope, because dialogue crosses clip boundaries. An envelope-only plan raises **no**
     confirmation (`plan_confirmation_description` :13748-13787 fires only on clip/track removal)
     and that is correct: it removes nothing.
128. **The two measurement rules, both stated because both bite.**
     1. `measure_mix_levels` refuses any clamped range under `LOUDNESS_GATING_BLOCK_FRAMES =
        19_200` sample frames — **400 ms at 48 kHz, 12 project frames at 30 fps**
        (`refuse_short_loudness_range`, export.rs:1404-1417). The planner therefore measures over
        the longest **flat** ducked span and the longest **flat** un-ducked span, each at least that
        many project frames.
     2. A merged span `[a, b)` with `b = span_end + hold_frames` produces a floor that is flat at
        `d` over exactly `[a, b]` — two `Linear` keys of **equal** value — because rule 126 places
        the attack ramp **before** `a` and the release ramp **after** `b`. A measurement window
        therefore fits when `span_length + hold_milliseconds >= 400` — **200 ms of speech at the
        200 ms hold default**, not the 950 ms of R13, which was derived for a curve whose ramps sit
        inside the window and does not survive rule 126's shape. The un-ducked window needs 400 ms
        of curve at `u`, i.e. a gap of at least `release_milliseconds + 400` between one span's `b`
        and the next span's `a − attack`. When no merged span and no gap qualifies, the planner
        reports `measured: null` with a `measurement_unavailable_reason` and **still commits the
        curve** — it does not refuse the plan; the curve is still correct and still worth
        committing. The fixture of rule 130 clears both by a wide margin: a 2.0 s burst gives a
        2.2 s floor and the 6.0–8.0 s stretch gives 2.0 s at `u`.
129. **Structured content** (hundredths and integers only): `{timeline_revision, music_track,
     dialogue_tracks, depth_tenth_db, attack_milliseconds, release_milliseconds, hold_milliseconds,
     range: {start_frame, end_frame} | null,
     parked_gain_tenth_db, ducked_gain_tenth_db, windows: [{start_frame, end_frame}],
     keyframe_count, measured: {unducked_lufs_hundredths, ducked_lufs_hundredths, delta_hundredths}
     | null, measurement_unavailable_reason: string | null, prepared_edit_plan{plan_id,
     expected_revision, preview}}`.
130. **The real-engine test**, on AU3's planner template
     (`au3_plan_audio_normalization_converges_through_the_real_engine`, mcp_server.rs:3838-3966): a
     12 s document with a music sine on track 1 and gated speech-shaped bursts on track 2, one burst
     from **2.0 s to 4.0 s** so the ducked floor is **2.2 s** long and one dialogue-free stretch
     from **6.0 s to 8.0 s**; invoke → assert the structured windows and
     `ducked_gain_tenth_db == parked + depth` → `commit_edit_plan` → assert the committed
     `TrackMix.gain_curve`'s key count and its first and last `at` → re-measure with
     `get_audio_levels` over frames **66..120** (ducked) and **180..240** (un-ducked) at 30 fps and
     assert the music track's integrated loudness differs by `depth` within
     `PLAN_DUCKING_DEPTH_BUDGET_HUNDREDTHS = 50` (0.5 LU), printing
     `AU4_DUCK depth=… unducked=… ducked=… delta=… budget=50`.

### 6.2 `plan_clip_fades` (Q6)

131. Evidence: each clip's head and tail measured **through the real mix path**. Because
     `measure_mix_levels` refuses a range under one gating block (rule 128.1), the window is exactly
     `LOUDNESS_GATING_BLOCK_FRAMES` **sample** frames — 19 200 = 400 ms at 48 kHz = **12 project
     frames** at 30 fps, spelled out because "exactly `LOUDNESS_GATING_BLOCK_FRAMES`" otherwise
     reads as project frames beside rule 126's frame arithmetic — and the decision reads the track
     stem's `true_peak_dbtp_hundredths`, which is reported for any non-empty programme however
     short. **A clip shorter than the window is skipped**, normative: `measure_mix_levels` returns
     an `Err` rather than a null on a range under one gating block
     (`refuse_short_loudness_range`, export.rs:1404-1417), so the planner skips such a clip with a
     **per-clip reason** in structured content and plans the rest, rather than failing the whole
     plan. A short-window RMS accessor is deferred to AU5, where repair work needs one anyway; the
     400 ms peak window is evidence-only and its worst failure is a 1-frame fade nobody needed.
132. Arguments: `{ tracks: Option<Vec<TrackId>>, threshold_dbfs_hundredths: Option<i32> (default
     -4_000), fade_milliseconds: Option<u32> (default 20) }`. For each media clip whose head window
     peaks above the threshold and whose `audio_fade_in_frames` is 0, propose
     `fade_in_frames = ceil(fade_ms × fps / 1000)` (**1 frame at 30 fps for the 20 ms default**);
     likewise for the tail. Emits **`SetClipAudio` only** — the existing gain and the other fade
     carried unchanged, and the pair clamped so `fade_in + fade_out <= clip_duration`.
133. It emits no new operation, adds no curve, and exercises §3.2 rule 54's separation of fades from
     envelopes for almost nothing. **It is the right thing to cut if Part B runs hot**: the row
     still reads as discharged without it, because the agent's half of "audio-aware trim behaviour"
     is a convenience and the operation half (§2.4 rule 26) is Part A.

### 6.3 Part B registry effect

134. +2 planners: `INSPECTOR_TOOL_NAMES` 78 → **80** (planners live in that array; `plan_*` infers
     `CapabilityKind::Planner` at runtime.rs:175-177, no `CAPABILITY_KIND_OVERRIDES` entry), so
     registry **132 → 134** with generated tools unchanged at 54. Both names are inserted in the
     array's existing alphabetical-within-family position beside `plan_audio_normalization`, and the
     ordering assert in `cc7_the_agent_surface_is_unchanged_by_this_slice` (:2537-2546) is extended
     rather than relaxed.
135. `server.rs:19478`'s `INSPECTOR_TOOL_NAMES.len()` assert moves to 80; both descriptions join the
     `< 1_024` byte budget list at server.rs:19487-19501; the **first sentence** of each carries
     what an agent must know before committing (`get_capability` and `search_capabilities` publish
     only `first_sentence(description)`, runtime.rs:184-190) — for ducking, that the plan replaces
     the music track's gain automation and that the measurement may be `null` on short windows.
136. The four ledger asserts, the derivation comment, the CC7 mirror test and one M36 row are
     regenerated by rule 87's procedure. The served quad stays `(7, 5_660, 3_510, 998)`.

## 7. Acceptance checklists

### Part A (A1–A21)

Closes exit-gate clause 1 verbatim from ROADMAP-AND-WORKFLOWS.md:648 — *"Envelope evaluation is
integer-exact and identical in both paths"* (A9–A13) — and clause 2 — *"edited envelopes survive
split/trim/slip/speed operations under a stated policy"* (A6–A8) — and the model half of the row's
deliverables (§1.2).

A1. All five fields absent serialise byte-identically to the pre-AU4 document string and a pre-AU4
    JSON deserialises with all five `None`; a curve-bearing document round-trips; both new
    variants join `document_and_every_operation_variant_round_trip_through_json` (contracts.rs:178)
    with `"curve":null` and with a curve; `serde_json::from_value` on `SetClipGainEnvelope` **errors
    on an omitted `curve`** and clears on `"curve": null`; `"curve"` is in each variant's schema
    `required` list and `parameter`'s schema carries `enum == ["gain_tenth_db", "pan_percent"]` —
    core `tests/contracts.rs`, `tests/au4_core.rs`, agent `schema.rs`.
A2. `TrackMix` is `Clone + PartialEq + Eq` and not `Copy`; `TrackMix::neutral` / `is_neutral`,
    `AudioMaster::is_neutral` and `AudioMix::is_empty` are still `const`; `is_neutral` is **false**
    for a neutral-scalar entry carrying a curve, and such an entry survives a round trip through
    `audio_mix` rather than being elided — core `tests/au4_core.rs`.
A3. Resolution: with a curve, the owner's value at frame `f` is `curve.value_at(f)`; without one it
    is the scalar; `value_at` returning `None` falls back to the scalar and never panics;
    `TRACK_AUTOMATION_PARAMETERS == ["gain_tenth_db", "pan_percent"]` and
    `ENVELOPE_DISPLAY_MIN_TENTH_DB == -400`, and **no validation path reads
    `ENVELOPE_DISPLAY_MIN_TENTH_DB`** — asserted as a named test that walks the `validate_document`
    and per-operation validators for the constant, not as a grep, because Part B's
    `envelope_value_to_y` does read it; **`envelope_coalesce_key(clip) == "envelope:{clip}"` and
    `track_automation_coalesce_key(track, parameter) == "track_automation:{track}:{parameter}"`
    land in Part A** with the two constants (rule 35); and `is_hold_only_parameter` is `pub`, is
    re-exported from `lib.rs`, and **AU4 adds no entry** to it or to `is_static_audio_parameter`
    (rule 38) — core `tests/au4_core.rs`.
A4. `SetClipGainEnvelope` validates **atomically** on the `clip_audio_operation_validates_bounds_
    fades_and_titles_atomically` template (contracts.rs:3560): an empty curve →
    `InvalidClipGainEnvelope`; a value at `-601` / `121` → `ClipGainEnvelopeOutOfRange`; a key at
    `clip_duration` → `ClipGainEnvelopeKeyframeOutsideClip`; unordered and negative keys → the two
    `AutomationCurveError` reasons; a title clip → `TitleClipHasNoAudio`, a freeze →
    `FreezeClipHasNoAudio`; `None` clears; **setting a curve equal to the stored one is accepted
    and produces a byte-identical document** (rule 27); every rejection leaves the document equal
    to `before` — core `tests/contracts.rs`.
A5. `SetTrackAutomation` validates the same way with `InvalidTrackAutomation`,
    `TrackAutomationOutOfRange` (pan at `101`, gain at `-601`),
    `TrackAutomationKeyframeOutsideProject` and `UnknownTrackAutomationParameter` (raised **before**
    curve validation); **`set_track_mix` merges**: a `SetTrackMix` on a track carrying both curves
    leaves both curves byte-identical, and a neutral `SetTrackMix` on such a track keeps the entry
    rather than retaining it away; **the entry lifecycle of rule 32a**: a `SetTrackAutomation` on an
    un-mixed track **pushes** a new entry and re-sorts by `track`, and a
    `SetTrackAutomation { curve: None }` clearing the last curve on an otherwise neutral track
    **removes** it, leaving the serialized document byte-identical to one that never carried a
    curve; setting a curve equal to the stored one produces a byte-identical document (rule 27); a
    hand-edited document with an over-running track curve is rejected by `Document::validate` —
    core `tests/contracts.rs`, `tests/au4_core.rs`.
A6. `rebase_clip_curve`: the boundary value is preserved **exactly** at both edges (both formulas,
    `value_at(delta_local)` and `value_at(delta_local + new_duration − 1)`); a `Hold` segment is
    exact everywhere; a `Linear` segment is within **1** tenth-dB; a truncated `EaseInOut` segment
    is **re-shaped**, pinned by the printed counterexample `AU4_EASED_RESHAPE frame=25 before=156
    after=250 delta=94` with the boundary key at frame 49 equal to **485** in both curves; a
    lengthening trim inserts nothing; an all-dropped curve keeps its single boundary key; the
    dedupe never picks a wrong value (a boundary key colliding with a survivor carries the same
    value) — core `tests/au4_core.rs`.
A7. `clamp_project_curve` and `recompute_duration`: deleting the last clip, right-trimming it,
    rippling it out, and speeding it up each **succeed** on a document whose music track, a bus and
    the master all carry curves keyed past the new duration, and each leaves the value at the new
    last frame unchanged; the **same** holds for a bus and a master `Effect.keyframes` curve, which
    previously failed with `AudioBusKeyframeOutsideProject` / `AudioMasterKeyframeOutsideProject`;
    **deleting the only clip** of a project whose music track, a bus, the master and a master
    `Effect.keyframes` all carry curves **succeeds**, and each curve is left as **one key at frame
    0** carrying the value that was audible at frame 0 (rules 17, 37.3); a document that does not
    shorten is byte-identical after the operation, and — pinning rule 18's guard — an
    `UpsertAudioBus` / `SetAudioMaster` / `SetEffectKeyframes` writing a key at `at >= duration` on
    an **unchanged** duration is still **rejected**, with `au2_core.rs:1128-1151` and `:1287-1297`
    green — core `tests/au4_core.rs`.
A8. One test per §2.4 row, each asserting **the audible value at the new boundary is unchanged**
    rather than merely that the document validates: `MoveClip`, `SplitClip` (both halves, and the
    regression that a split of a clip carrying both an envelope and a colour curve leaves both
    halves evaluating to the same values at the same **project** frames as the original did),
    `TrimClip` both edges, `SlipClip` (untouched), `SlideClip`, `RollEdit`, `ReplaceClip`,
    `SetClipSpeed`, `RippleDeleteClip` (the `start − 1` boundary key, the dropped window, the
    shifted `ripple_point` key landing at `start`), `RippleInsertGap` (a key exactly at `at`
    shifts; nothing is inserted), `DeleteClip`, `RemoveTrack`, `RemoveAudioBus`, and **`RelinkAsset`
    to a candidate asset with a different `fps`** (every clip whose project duration shortened has
    its envelope, its clip-effect curves and its audio fades clamped, and the operation succeeds).
    A **left** `SlideClip` and a **left** `RollEdit` are included explicitly, with the negative
    `delta_local` of rule 13: nothing is inserted on the left and the newly exposed head is flat at
    the first key's value. **No bus or master curve moves under `RippleDeleteClip` or
    `RippleInsertGap`** (rule 24), asserted byte-identical. **The speed round trip is destructive
    and is pinned as such** (rule 25): 100 % → 200 % → 100 % drops the keys past the halved
    duration and does not restore them, while a single `Command::Undo` does. Plus: a shortening
    trim over a keyframe **succeeds** where it previously returned `EffectKeyframeOutsideClip`,
    while a **written** over-running curve still fails; a split of a fully-faded clip succeeds and
    gives the left half the fade-in and the right half the fade-out with the cut side zeroed; a
    trim shorter than `fade_in + fade_out` clamps instead of raising `AudioFadesTooLong` — core
    `tests/au4_core.rs`, `tests/contracts.rs`.
A9. `automation_step`: the two anchors; `t` at the first, middle and last sample of a frame;
    `t == 0.0` when `s1 <= s0 + 1`; a negative local frame handled before `frame_to_samples`;
    fallback to the static value with no curve and on `value_at` overflow; **`Hold` is flat** —
    `{10: A, Hold}, {100: B}` reads `A` at every sample of frames 10…99, and the printed
    `AU4_HOLD_STEP boundary=160000 reached=160239 exact=160240` at 30 fps / 48 kHz with
    `automation_hold_declick_frames(48_000) == 240` and one project frame **1 600** samples; **the
    declick has no seam** — its last sample is `to_bits()`-identical to the plain value, printed as
    `AU4_HOLD_SEAM reached=… bits_equal=true`, and its interior is strictly between `h` and `target`
    and monotone in `project_sample` (rules 42, 45); both lerps are `a + (b − a) * t` with
    `to_bits()` equality at `t == 0` and for `a == b` at every `t`, and the automated pan arm
    multiplies in `track_stage_parameters`' own order, `gain * pan[ch]` (rule 43);
    `pan_channel_ratios(ConstantPower, 0) == [FRAC_1_SQRT_2; 2]`; the printed `AU4_PAN_CHORD` sweep
    asserts **`measured <= sagitta <= bound`** at every integer pan step — at `Δ = 10` a measured
    deviation no greater than the sagitta **7.710e-4**, itself no greater than the bound
    **3.083e-3** — because the measured per-channel deviation is a *component* of the sagitta vector
    and is strictly smaller everywhere except at the mid-angle — core `automation.rs`, media
    `audio.rs`.
A10. Per-project-frame evaluation against a per-sample reference, **two asserts** on the AU2
     template (`per_project_frame_evaluation_matches_a_per_sample_reference`, audio.rs:5464):
     bit-identity at every project frame's **first sample** against `db_gain(value_at(f))` by
     `to_bits()`, and monotonicity between the two anchors across the frame's interior, for gain and
     for pan under **both** laws — media `audio.rs`.
A11. A document carrying **all five** owners' curves: `mix_audio` twice is `to_bits()`-identical,
     the stems path and the clamped observer feed agree, and the SHA-256 of the little-endian mix
     bytes is **printed** as `AU4_AUTOMATION_SHA256` beside AU3's family (audio.rs:7158-7224) rather
     than pinned across operating systems — media `audio.rs`.
A12. `assert_playback_matches_export` (audio.rs:4155-4198) on a new `parity_document_with_automation`
     — a clip envelope, both track curves, a bus fader curve and a master fader curve — passes its
     nine windows at `1.0e-6` and its frame-5 seek arm; the windowed `mix_levels` regression of
     rule 67 reads the same values as the full-range pass restricted to the same window — media
     `audio.rs`, `export.rs`.
A13. **Neutrality**: a curve-free document exports **byte-identically** to pre-AU4 through
     `to_bits()` on the whole buffer; `TrackStageRuntime::apply`'s and `GainRamp::apply`'s fast
     paths still fire; every new serialized default is omitted; and **the envelope is not gained
     twice** (rule 52) — a clip with `audio_gain_tenth_db = -60` **and** an envelope mixes
     `to_bits()`-identically to the same clip with `audio_gain_tenth_db = 0` and the same envelope,
     because `constant_gain` is `1.0` whenever a curve exists — media `audio.rs`, core
     `tests/au4_core.rs`.
A14. **Latency, per owner**: on a document declaring `CHAIN_LOOKAHEAD_MILLISECONDS` on **both**
     stages, the measured offset between an automated **clip envelope** key frame and the audible
     result is exactly **0**, and likewise for a **track** gain and pan curve; the measured offset
     for an automated **bus** fader equals that bus chain's own node latency, and for the **master**
     fader equals `stage_latency_frames(L_bus, 48_000)` = **960** frames; all four are **0** on a
     document declaring no lookahead; every case is printed as
     `AU4_LATENCY owner=… declared_ms=… offset_frames=…` — media `audio.rs`.
A15. **Live path**: `live_audio_change` truth table over `Do`, `DoBatch`, `DoBatchCoalesced`, `Undo`,
     `Redo`, `None`, an empty batch, a lookahead change, and every mix of the two operation sets,
     yielding `None`/`Mix`/`ClipShaping`/`Both`; a `SetClipGainEnvelope` batch **retargets instead
     of re-cueing** and is audible without a stop; `update_clip_shaping` rebuilds in place on a
     matching segment layout and returns `false` (falling back to re-cue) when the layout changed;
     `Playback::update_clip_shaping` defaults to `set_document` on a counting double; **one
     `Control::UpdateAudio(kind, document)` carries both halves** and the `Both` arm applies mix
     then shaping from the same `Arc<Document>` (rule 75); a plain **`SetClipAudio`** gain drag also
     retargets instead of re-cueing, which is the AU1 debt rule 77 pays off; a **`SetPanLaw`** on a
     document with one automated and one non-automated track **retargets both stages** and is
     audible in the next chunk without a re-cue (rule 59); a **curve edit applied mid-project-frame
     is audible on the next sample frame** — the per-curve memo does not hold the pre-edit anchors
     for the rest of the frame (rule 46); and an **edit made while an automated track is silent**
     leaves `current` at the automated value, so the first audible chunk of the next clip does not
     ramp from the parked scalar (rule 60) — app `app.rs`, media `audio.rs`, `engine.rs`, core
     `media.rs`.
A16. **Telemetry is derived, never published** (§4.5): the displayed automated value for a fader
     is `curve.value_at(frame)` at `Playback::position()`'s audible frame and equals the integer the
     DSP used at that frame (up to rule 65's per-owner offset); `Playback` gains **no** new field
     and **no** new atomic, `Analysis` gains **no** variant and therefore
     `BaselineProofAnalysis`'s forwarding arms are unchanged — asserted on AU3's forwarding-arm
     test template, so a later addition to `Analysis` cannot slip past without one; and no write,
     touch or latch surface exists — core `media.rs`, app `mixer_ui.rs`.
A17. `render_*`: the pre-AU4 compact golden is byte-unchanged on a curve-free fixture; a
     curve-bearing fixture renders `envelope:[…]` on the clip line, `gain_curve:[…]`/`pan_curve:[…]`
     in `mix=`, and `gain_curve=[…]` on the bus and master lines, in `render_effects`' `at:value:
     Interp` spelling; the two parameter tokens and the two render keys are derived from
     `TRACK_AUTOMATION_PARAMETERS`; a curve-bearing bus round-trips through `render_audio_mix` →
     `upsert_audio_bus`; and **whole-owner-set semantics are pinned** (rule 34) — an
     `UpsertAudioBus` that **omits** `gain_curve` on a curve-bearing bus **clears** it, exactly as
     an
     omitted `effects` clears the chain, and the same for `SetAudioMaster` — agent `render.rs`,
     `tests/mcp_server.rs`.
A18. `get_audio_levels` on a curve-free document is byte-unchanged; on a curve-bearing one
     `TrackLevels.mix` carries both keys and every leaf of the serialized report is an integer,
     boolean, string or null — agent `tests/mcp_server.rs`.
A19. Registry Part A: **54** generated, **78** inspectors, **132** registry; `INSPECTOR_TOOL_NAMES`
     unchanged; the served quad byte-identical at `(7, 5_660, 3_510, 998)`; the four ledger asserts
     regenerated from the printed `ToolSurfaceMetrics` and the derivation comment extended with a
     per-part split that sums exactly; `cc7_the_agent_surface_is_unchanged_by_this_slice` and its
     doc-comment ledger updated; both new tools **and `set_effect_keyframes`** carry the
     `.idempotent(...)` flag (rule 84) — agent
     `schema.rs`, `server.rs`, `tests/mcp_server.rs`.
A20. QC is unchanged: a curve-bearing document produces the same `qa_document` issue set and the
     same `get_audio_qc` exception codes as the same document without the curve, given the same
     signal — core `qa.rs`, media `audio.rs`.
A21. `CHANGELOG.md` carries the `### Added` line, **all three** `### Changed` lines — the two
     reject → clamp changes and the destructive speed increase of rule 25 — and **both** `### Fixed`
     lines; `MEDIA-POLICY.md` carries the `automation_step` paragraph; the AU1
     §3.4 and AU2 §2.2 pointer sentences are present; the M36 Part A row is present — docs, pinned
     by `include_str!` string pins.

### Part B (B1–B14)

Closes the row's remaining deliverables — the rubber-band editor on the clip, bus automation editing
in the mixer, and the agent planners (§1.2).

B1. Pure mappings, no window: `envelope_x_to_local_frame` / `envelope_local_frame_to_x` round-trip
    every frame of a 300-frame clip in a 240 px rect; `envelope_value_to_y` / `envelope_y_to_value`
    put `0` tenth-dB at **23.1 %** from the top and clamp `-401` and `121` to the edges; the three
    derived resolutions — **13.7**, **27.5** and **52** tenth-dB per logical pixel — are asserted
    **within ±0.1** and printed, because the three band heights are exactly **38.00**, **18.88** and
    **10.00** px (so 520/18.88 = 27.54 and an equality assert would fail); every mapping takes rule
    95a's `envelope_band_rect` return, and `envelope_band_rect` gives `None` for a Video, Title or
    Freeze clip — app `timeline_ui.rs`.
B2. `envelope_hit` finds a key within `9.0` and nothing at `9.1`; **no interact is allocated** when
    the pointer is further away or when `duration × pixels_per_frame < 24.0`; the envelope interact
    is registered **after** `body`, `left` and `right` and its rect never overlaps either 6 px trim
    handle in x; a pointer 3 px from the line inside the left handle's x band still reaches the
    handle — app `timeline_ui.rs`.
B3. Gesture rules as pure functions: insert at a snapped clip-local frame takes the curve's current
    value there; a dragged key is constrained between its neighbours ±1 frame and clamped to
    `0..=duration − 1` in x and `-600..=120` in y; remove drops one key; **removing the last key is
    refused**; **Alt bypasses snapping on envelope keys** through the existing frame-level flag, and
    a snapped insert lands on a project frame converted by subtracting `timeline_start`, never by
    re-deriving x from `pixels_per_frame` (rules 96, 103); every rule returns the whole key list —
    app `timeline_ui.rs`.
B4. One rubber-band drag is **one undo entry**: N `dragged()` frames plus the `drag_stopped()` frame
    coalesce under `envelope:{clip}#{gesture}` and a single `Command::Undo` restores the pre-gesture
    document, on the `a_coalesced_bus_fader_drag_is_one_undo_entry` template (au2_core.rs:1384); a
    frame whose recomputed curve equals the document writes **nothing**; a discrete inspector edit
    in the same frame drops the key; and — the regression risk of grafting `InspectorEdits` into
    `timeline()` — a **clip move, a trim, a marker drag and a playhead drag each still emit exactly
    one batch at `drag_stopped` and nothing on the intermediate frames** (rule 105) — app
    `timeline_ui.rs`, `inspector_ui.rs`.
B5. A headless painted frame of the timeline with an envelope-bearing clip writes **no** operation
    and paints the polyline and its points (`painted_text` / shape walk, timeline_ui.rs:2192-2207)
    at `ENVELOPE_POINT_RADIUS = 3.5` and `ENVELOPE_STROKE = 1.6`, in **`ACCENT` when the clip is
    selected and `TEXT_PRIMARY_64` otherwise** (rule 102) — app `timeline_ui.rs`.
B6. The `Envelopes` toolbar toggle is session state, defaults on, and when off the overlay paints
    nothing and allocates no interact; toggling it emits no operation — app `timeline_ui.rs`.
B7. The fader and pan **rails are disabled** while their curve exists and read the automated value
    at the audible position; the numeric readout stays editable and a typed value writes the parked
    scalar through `track_mix_operation`; `reset_row_keeps_automation` — clicking `Reset` on an
    automated track clears the five scalars and leaves both curves — and `reset_row` is not drawn at
    all when only the curves are non-neutral; the amended hover text is painted — app `mixer_ui.rs`.
B8. The `A` chip paints inline after the kind icon; the **worst case — a `NO AUDIO`, `SILENCED`,
    off-neutral, automated track, three extra rows and the chip** — is added to
    `a_track_strip_fits_the_mixer_dock` and measures **`<= 240.0`, the only binding assert** (the
    `232` figure is not pinned; the measured height is printed and recorded as a §0 erratum); and
    `caps_label_width("A")` fits the 72 px strip beside the caption and icon — app `mixer_ui.rs`.
B9. The chain pane's `AUTOMATION` section lists `Fader` plus exactly the non-hold-only,
    non-static parameters of the selected chain (`is_hold_only_parameter` excludes `bypass`,
    `detector`, `true_peak`; `is_static_audio_parameter` excludes the two `lookahead_milliseconds`
    and `rms_window_milliseconds`); `+ Key at playhead` and `Clear` fold into exactly **one**
    `UpsertAudioBus` / `SetAudioMaster` per chain per frame; `parameter_value` resolves through
    `Effect::integer_parameter_at` at the audible frame so a keyframed card no longer shows its
    neutral; `measure_automation_section` is within `1.0` of its `MEASURED` and under its
    `const BUDGET: f32 = 120.0` with an empty and a scrolled ten-key curve; **and the pane itself is
    asserted** — a master pane carrying both the `AUTOMATION` section and the 90 px-budget
    `LOUDNESS` section measures under its own named `BUDGET`, at ±1 px, with an empty and a scrolled
    ten-key curve, and still fits a 260 px dock and scrolls — app `mixer_pane_ui.rs`,
    `mixer_ui.rs`.
B10. The inspector `ENVELOPE` block paints the key list in tenth-dB, writes `SetClipGainEnvelope`
     with `curve: None` from `Clear`, coalesces a value drag under `envelope:{clip}`, and the
     painted frame writes no operation otherwise; the inspector's clip-gain slider is **not
     disabled** while a curve exists (rule 119) — it carries the `KEYFRAMED` badge and writes the
     parked scalar, unlike the mixer rail of rule 109 — app `inspector_ui.rs`.
B11. `plan_audio_ducking`: three direct tests (window construction and the four `Linear` keys per
     merged span at **5 / 12 / 6** frames for the 150 / 400 / 200 ms defaults at 30 fps; the
     relative keying — `ducked == clamp(parked + depth, -600, 120)` on a track parked at `-600` and
     at `120`; the refusal text `"track {t} already carries a gain curve; clear it or pass replace:
     true"` and that `replace: true` proceeds) plus the **real-engine convergence test** of rule 130
     with its printed `AU4_DUCK` line and the `50`-hundredth budget; a fourth case — **a document
     whose only speech span is 100 ms, so `span + hold = 300 ms < 400 ms`** — where the plan still
     commits and reports `measured: null` with its reason (rule 128.2); a **`range`** case: a span
     outside `[range.start, range.end)` is not considered, the emitted curve is clamped to that
     window, the measurement is **not** restricted by it, and `range` is echoed in structured
     content (rules 126, 129); and an envelope-only plan raises **no** confirmation
     (`plan_confirmation_description` fires only on clip or track removal, rule 127) — agent
     `server.rs`, `tests/mcp_server.rs`.
B12. `plan_clip_fades` proposes `SetClipAudio` fade frames only, never a curve, only for clips whose
     head or tail window peaks above `-4_000` hundredths, never overwrites a non-zero fade, and
     clamps the pair to the clip duration; the window is **19 200 sample frames = 400 ms at 48 kHz
     = 12 project frames at 30 fps**, asserted in those terms; and a **clip shorter than the
     window** is **skipped with a per-clip reason** in structured content while the rest of the plan
     commits, rather than the `Err` from `refuse_short_loudness_range` failing the whole plan
     (rule 131) — agent `server.rs`, `tests/mcp_server.rs`.
B13. Registry Part B: **80** inspectors, **134** registry, **54** generated; the ordering assert
     extended; both descriptions under `1_024` B with the required clause in the **first** sentence;
     the served quad byte-identical; the figures regenerated; one M36 row — agent `schema.rs`,
     `server.rs`, `tests/mcp_server.rs`.
B14. DESIGN.md contains §5.6's Timeline and Mixer sentences including "the band is the coarse
     gesture and the inspector's keyframe list is the exact one" and rule 110's two sentences; the
     **new** `timeline_ui.rs` `include_str!` test slices `### Timeline`; `CHANGELOG.md`, the
     `ROADMAP-AND-WORKFLOWS.md` status paragraph naming the two parts and the clause split,
     README line 36 and the M36 Part B row are present — app `timeline_ui.rs`, `mixer_ui.rs`, docs.

Exit gate for each part: its checklist green in `cargo test --workspace` locally on Linux, `cargo
fmt --check` and `cargo clippy --workspace --all-targets -- -D warnings` clean, both CI operating
systems green after push, and both review passes recorded. Part B additionally needs one hands-on
session on a real project — a rubber-band ride drawn on a clip during playback without a re-cue, a
bus fader curve edited in the chain pane, and one `plan_audio_ducking` committed and listened to —
read by Riel. Part A lands as **`feat: complete AU4a automation model`**; Part B as **`feat:
complete AU4b automation editing`**.

## 8. Files

**Part A.** *Core:* `automation.rs` (`holds_at`, `hold_step_at`, `HoldStep`, `rebase_clip_curve`,
`clamp_project_curve`); `model.rs` (the five fields, `TrackMix` loses `Copy`, the two widened
`is_neutral`, `recompute_duration`'s clamp loop, `TRACK_AUTOMATION_PARAMETERS`,
`ENVELOPE_DISPLAY_MIN_TENTH_DB`); `operation.rs` (the two variants and their `apply` arms, the nine
`OpError` variants, `set_track_mix`'s merge, `split_clip`, `trim_clip`, `slide_clip`, `roll_edit`,
`replace_clip`, `set_clip_speed`, `ripple_delete_clip`, `ripple_insert_gap_for_tracks`,
`validate_document`'s new per-owner checks, `is_hold_only_parameter` becomes `pub`); `qa.rs` (A20's
test only); `lib.rs` (re-exports); `tests/contracts.rs`; new `tests/au4_core.rs`. Literal sites
below are **grep counts re-counted on de0beb3, word-bounded, `target/` excluded** — they bound the
work and over-count it, because each includes the `pub struct` definition and roughly half the
`TrackMix {` occurrences use `..` struct-update syntax and need no edit (rule 4). In core: **4**
`TrackMix {` in `model.rs`, **1** in `operation.rs`, **2** in `tests/au1_core.rs`, **11** `Clip {`
in `qa.rs`, plus the `AudioBus {` / `AudioMaster {` sites in `model.rs`, `operation.rs` and the four
`tests/*.rs`.
*Media:* `audio.rs` (`AutomationStep`, `automation_step`, `AUTOMATION_HOLD_DECLICK_MILLISECONDS`,
`automation_hold_declick_frames`, `ClipAudioShaping` loses `Copy` and gains the curve,
`TrackStageRuntime` fields / `apply` / `retarget`, `GainRamp` fields / `apply` / `retarget`, the two
`mix` fader call sites, `AudioMixSource.clip`, `AudioMixer::update_clip_shaping`, the per-frame
amplitude memo, tests); `export.rs` (`mix_pass`'s shaping call and rule 67's comment,
`measure_mix_levels`); `engine.rs` (`Control::UpdateAudio(LiveAudioChange, Arc<Document>)`, the
worker arm, `Playback::update_clip_shaping`); `timeline.rs` (**8** `Clip {`); `lib.rs`;
`tests/generated_media.rs` (**11** `Clip {`). Media also carries **2** `TrackMix {` sites in
`audio.rs`.
*Agent:* `schema.rs` (two `operation_tool_name` arms, two descriptions, the idempotent list, the
name pins); `render.rs` (**6** `Clip {` and **2** `TrackMix {`, `render_clip_audio`,
`track_mix_fields`, the bus and master lines, the goldens); `server.rs` (**10** `Clip {`, the ledger
figures and derivation comment);
`eval.rs` (**5** `Clip {`); `tests/mcp_server.rs`.
*App:* `app.rs` (`LiveAudioChange`, `is_audio_mix_operation`, `is_clip_audio_operation`,
`live_audio_change`, the single `Control::UpdateAudio` send site; **0** `TrackMix {` literals);
`mixer_ui.rs` (`track_mix_operation`/`track_mix_toggle_operation` lose `const`, the by-value
`TrackMix` parameters become `&TrackMix`, **16** raw `TrackMix {` sites);
`inspector_ui.rs` (**9** `Clip {`); `timeline_ui.rs` (**7** `Clip {`). *Docs:* this file,
`CHANGELOG.md`, `MEDIA-POLICY.md`, `AU1-MANUAL-MIX.md`, `AU2-EQ-AND-DYNAMICS.md` (pointers),
`M36-AGENT-RUNTIME-EFFICIENCY.md`.

**Part B.** *App:* `timeline_ui.rs` (the envelope constants, the four pure mappings, `envelope_hit`,
the interact and its allocation order, the paint layer, the `Envelopes` toggle, the `InspectorEdits`
graft, the first `include_str!` test); `mixer_ui.rs` (`caption_row`'s `A` chip, `meters_and_fader`
and `pan_control` disabled rails and editable readouts, `reset_row`'s second early return and
tooltip, the strip measurement, the DESIGN.md pins); `mixer_pane_ui.rs` (`automation_section`,
`measure_automation_section`, `parameter_value` at the audible frame, the EQ well's frame);
`inspector_ui.rs` (the `ENVELOPE` block); `theme.rs` (only if a token is needed; the three curve
literals are reused, not tokenised). *Agent:* `schema.rs` (`INSPECTOR_TOOL_NAMES` 78 → 80, two
descriptions); `server.rs` (the two planner handlers and their dispatch arms, the argument structs,
the ledger figures); `tests/mcp_server.rs`. *Docs:* this file, `DESIGN.md`, `CHANGELOG.md`,
`README.md`, `ROADMAP-AND-WORKFLOWS.md`, `M36-AGENT-RUNTIME-EFFICIENCY.md`.
