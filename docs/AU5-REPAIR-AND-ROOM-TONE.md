# AU5 Repair and room tone

Status: contract, promoted 2026-09-10 from revision 2; working files in target/review/au5/.
Revision 1 was the contract as ruled by `orchestrator-notes.md` **N1**; this is that same document
**as amended by `critic-contract.md` F1–F25 under N2**, which accepts every finding with the critic's
exact replacement text and rules on all three of revision 1's open notes. §0 gains implementation
errata as the code lands, as AU2 §0, AU3 §0 and AU4 §0 did. Built from the AU5 design brief
(`design-brief.md` F1–F19, D1–D14, §4's numeric contract, §5's fixtures, §6–§12) as amended by its
critic report (`critic-brief.md` B1–B4, S1–S14, nits 1–16 and its answers to Q1–Q10), and by
`orchestrator-notes.md` N0, N0.1, N0.2, N0.3, **N1** and **N2**, against main at af43232. N1 and N2
accept every critic finding as written unless they amend it, and where a ruling and an earlier
document disagree the ruling wins; N1 also **withdraws N0.3(c) and N0.3(d)** in favour of D6 and D8
as critiqued. Every finding is folded into the rule, table cell or checklist item it belongs to and
registered in §0 as R1–R49; nothing is carried as an appendix. The numbers in this document are the
contract; implementation follows them or amends this file first.

AU5 is the fifth slice of the audio programme (ROADMAP-AND-WORKFLOWS.md:649). It lands as **one
contract in two parts, two commits**: **Part A — Repair nodes and measurement** (three descriptors
and their static/hold-only/latency rules, the in-house inverse FFT and its twiddle table, the three
runtime state machines, three new `Analysis` measurements, `core/src/audio_repair.rs`, the
`get_audio_repair` inspector, and the synthetic-corruption fixtures with their pinned gains) and
**Part B — Room tone and repair planners** (the room-tone store and `capture_room_tone`, gap
enumeration and the fill arithmetic, `plan_room_tone_fill`, `plan_dialogue_repair` and its
amendment to AU3's normalization planner, the three Mixer cards, the `Learn` gesture, the timeline
`Room tone` button, the seam pin and DESIGN.md). Part A lands first and is a complete exit gate on
exit-gate clause 1 on its own. Each part runs the full workspace gate and both review passes on
every crate it touches, core included.

## 0. Errata

The implementation records each deliberate deviation from the text below here, as AU2 §0, AU3 §0 and
AU4 §0 do. Four figures are regenerated as the code lands: Part A's registry quad after
`get_audio_repair`, Part B's after the three new capabilities, and the `MEASURED` constants beside
the three card height budgets and the new bus-pane budget (§6.5).

**Resolved before implementation (N1, on `critic-brief.md` B1–B4, S1–S14, nits 1–16 and Q1–Q10):**

- **R1 (B1).** `node_lookahead_milliseconds` (audio.rs:1361-1373) **deletes its hard-coded neutrals**
  and reads the descriptor exactly as core's `chain_lookahead_milliseconds` (model.rs:735-755) does.
  A Part A test pins media's per-node read against core's chain sum for **every** audio descriptor
  (§3.6, A2).
- **R2 (B2).** `max_click_milliseconds` is `0..=1`, the post-span guard is **1 ms**, the declaration
  stays **3 ms**, and a construction assert walks the parameter's **whole domain** at 44.1 / 48 /
  96 kHz (§3.5 rule 55, A6).
- **R3 (B3).** The profile's wire unit **is** `band_level_hundredths`' band level — a full-scale sine
  reads 0 — in tenth-dB; the runtime conversion to a per-bin floor power is spelled once with its
  derivation and pinned by a band-limited-noise fixture. The measurement window stays 4 096 / 2 048
  (§3.3, A4).
- **R4 (B4).** All 31 profile neutrals are **−1200**, plus the runtime rule "all 31 at the neutral ⇒
  unity gain"; the `noise_profile_missing` QA warning stays as advice (§2.1 rule 5, §2.3 rule 18,
  §3.2 rule 41).
- **R5 (S1).** §11's `forward_fft` deferral is **lifted**: a `OnceLock` twiddle table per **stage
  size**, bit-identical to today's arithmetic, plus a printed `AU5_PREROLL` lane. Skipping block work
  during preroll is **not** adopted — it would desynchronise the OLA state (§3.1, A5).
- **R6 (S2).** One rule: *a static parameter takes no keyframe; the runtime reads static parameters at
  construction and re-derives them on every `parameter_epoch` bump, except those in `node_structure`,
  which force a rebuild.* `max_click_milliseconds` is **not** static — the detector is sized from the
  descriptor **maximum**. AU2's reason string becomes **"is read when the chain is built or retuned
  and cannot be keyframed"**, an AU2 §0 erratum applied at its three pins (§2.2, A3).
- **R7 (S3).** The de-click reference is a **trailing** mean-square window that **excludes flagged
  samples**; the arithmetic proving the fixture detects lives in the doc comment beside the budget
  (§3.5 rule 53, A6).
- **R8 (S4).** Clicks at `4_800 × (k + 1)` for **`k in 0..19`** — 19 of them, the last ending at frame
  91 208 of 96 000 — and the count is pinned exactly (§3.11(c), A9).
- **R9 (S5).** D6(iii) re-worded: the single-flight async machine is paid anyway by §6.3's `Learn`
  button and already exists; what N0.3(c) really costs is the **cache**. app.rs:1258-1266 is **one**
  100 ms arm over three predicates, and the app is not spinner-less. The conclusion stands (§3.7
  rule 62).
- **R10 (S6).** `timeline_silences` is fed by an async per-asset analysis. Both `Learn` callers gate
  on readiness exactly as `plan_audio_ducking` does (server.rs:8559-8566, direct call :8581), with a
  **second refusal arm** separating "not analysed yet" from "no span is long enough" (§5.6 rule 107,
  §6.3 rule 128, B9).
- **R11 (S7).** `normalization_context` returns **extend bus N** — id, name, `tracks`,
  `ducking_sidechain_tracks`, `gain_curve` from the existing bus, whose `tracks` must **equal** the
  requested set — `normalization_bus` **appends** after the repair prefix, and the prefix rides all
  four convergence iterations. The in-crate refusal pin (server.rs:26140-26144) is updated; nothing
  under `crates/kinewright-agent/tests/` pins it, so AU3's baselines are untouched. A test runs both
  planners **in either order** and lands at exactly 20 ms (§5.7, B11).
- **R12 (S8).** Core gains an inverse of `map_source_range_to_project`; the fill asserts
  `clip_duration(fill) == gap`, shrinks by one asset frame on overflow, never overlaps, and skips a
  gap with a per-gap reason when no source range maps exactly. The seam fixture gains a **25 fps** arm
  and a project-faster-than-asset arm (§5.3, §5.4, B6).
- **R13 (S9).** `track_gaps` is **leading + interior** only, content- and kind-agnostic, never
  trailing; `qa_document` is refactored onto it with its output pinned **identical** on the existing
  corpus; the planner filters to audio itself (§5.2, B3).
- **R14 (S10).** `has_gain_computer` is hoisted to **core**; both byte-identical copies
  (audio.rs:222-228, mixer_pane_ui.rs:1261-1266) are deleted and both callers read the core one, which
  gains `audio_denoise` (§4.4, A13).
- **R15 (S11).** `eq_well` is private and hard-wired to `parametric_eq_magnitude_db`; AU5
  **parameterises the magnitude source** and nothing else. The comb reuses the well whole; the noise
  well is a **new** 31-bar chart on a −120…0 dB scale sharing only chrome; the bus-pane pin is a
  **new** test (§6.2, §6.5, B14).
- **R16 (S12).** "No UI in Part A" means no new editing **surface**, not "no app code": Part A carries
  the three `BaselineProofAnalysis` forwarding arms with their own tests, and the **SNR-gain lane** on
  `get_audio_repair` against `AUDIO_REPAIR_SNR_GAIN_BUDGET_HUNDREDTHS`, so clause 1 is pinned in
  Part A in the row's own words (§4.5, §3.11(d), A11, A17).
- **R17 (S13).** The hum Q row is **`notch_q_hundredths`**: `mixer_unit` tests `_q_hundredths` → Q
  *before* `_hundredths` → Ratio, so a bare `q_hundredths` would have read "12.0:1" (§2.1, B13).
- **R18 (S14).** `get_audio_repair`'s **first sentence** and the report prose both say the floor is a
  **percentile** and name the direction of its bias. Hum shoulders at `f × 2^(±1/6)`, Goertzel block
  **≥ 4 800** samples, excess **summed** over harmonics with the per-harmonic values reported
  (§2.4 rule 21, §3.9, §4.1, A10).
- **R19 (nits 1–4).** Scratch is **≈ 34 kB** stereo, not 8 kB. The OLA delay is **`window − 1`**, so
  the pads are **65 / 18 / 129**, pinned by an **impulse** rather than by arithmetic. The Hann²
  constant **is** exactly 1.5. "11 ms fails at 44.1 kHz" is right in conclusion, wrong in reason — it
  halves the window to 256 rather than under-declaring a derived delay (§3.2, A5).
- **R20 (nits 5–6).** §3.11(b)'s 300 Hz loss is **≈ 0.13 dB**, not 0.05 (margin 3.8×, nothing moves),
  and the ≈ 30 dB depth is the **exactly-on-centre** case. §3.11(c)'s Catmull-Rom bound is a **peak**
  error against an **RMS** signal — the honest drop is ≈ 88–91 dB against a 30 dB gate — and
  `rms(corrupt − clean)` is printed, not predicted, because the error is `0.9 − x[n]`, not 0.9.
- **R21 (nits 7–8, amended by R34).** `tone`, `pseudo_random_amplitude`, `rms` and `wav_f32` are
  promoted from `audio.rs`'s `#[cfg(test)]` module to `test_support.rs` **with their signatures
  unchanged** (R45); `nearest_rank_index` (loudness.rs:219-222), `hann_window` (spectrum.rs:253-267)
  and **`SILENCE_POWER` (spectrum.rs:36)** become `pub(crate)`, the last because rule 65 and rule 68
  name it from `export.rs`, a different module in the same crate (§3.11, §8).
- **R22 (nit 9).** `INSPECTOR_TOOL_NAMES` is the whole **hand-written capability** list;
  `capture_room_tone` is an **Action** and the two planners **Planners** by `CapabilityKind` inference
  (runtime.rs:165-181). 80 → 81 → 84 and 134 → 135 → 138 stand; "inspectors 80 → 84" does not. The
  served quad is `(7, 5 660, 3 510, 998)` and is unchanged in both parts (§4.3, §5.9).
- **R23 (nit 10, amended by R46).** `effect_documentation` rows measure **≈ 39 B** and a **profile**
  row **≈ 49 B**, so the unhatched cost is `31 × 49 + 14 × 39 ≈ 2 065 B` per spliced tool ≈
  **10.3 kB**, not 8.8 kB — the hatch argument is strengthened, not weakened; `render_effects` spells
  31 profile rows as **898 B** against **170 B** compact; and `render_param` quotes `Text` through
  `{value:?}`, so the compact spelling needs **its own arm**. Every byte figure here is an **estimate
  to be measured and re-derived** (§4.2, §4.3).
- **R24 (nits 11–12).** `ClipContent::` measures **265** sites, not 281. Document edits leave the
  Mixer through `InspectorEdits` (mixer_ui.rs:503, :519-528, :574); `MixerFrame` (:471-478) carries
  session and telemetry only — so `learn_noise_profile` is right **because it is a request** (§6.3).
- **R25 (nits 13–14).** There is **no bus-pane height pin today**, so AU5's is a **new** test. The
  timeline toolbar measures **≈ 687 px** ripple-off and **≈ 739 px** with the `RIPPLE` label, with no
  wrap and no scroll, and `Fit` already saturates at its 240 px floor (§6.4, §6.5, R47).
- **R26 (nits 15–16).** `AddAsset` needs a **`name`**, and `probe_path` refuses a duration rounding to
  **zero frames** (decode.rs:114-119). `INSERTABLE_AUDIO_EFFECTS`' `debug_assert` is **replaced**, not
  re-worded, by one that can fail (§5.1, §5.8, §6.1 rule 119, B2, B13).
- **R27 (Q1, Q2, Q7).** Synchronous mix-path profile with R10's gate and R3's unit; **STFT**, with the
  third-octave IIR cascade named as the fallback and nowhere else; **31 inline** static parameters
  with R4's neutral, the 15-half-octave variant kept in reserve (§2.1, §3.2, §3.3, §3.7).
- **R28 (Q3, Q4, Q6).** `CHAIN_LOOKAHEAD_MILLISECONDS` stays **20** — the 15 ms sum survives only
  because de-click stays at 3 ms (R2). `plan_audio_normalization` **is** amended per R11; hum removal
  adds **no** new `BiquadCoefficients` constructor (§2.3 rule 16, §3.4, §5.7).
- **R29 (Q5, Q8).** Same-track butt join with R12's arithmetic and R13's gap definition; a later trim
  into a filled gap fails with `ClipOverlap`, exactly as trimming into any neighbour already does. The
  timeline `Room tone` button ships and is **the first thing cut if Part B runs hot** (§5.3, §6.4).
- **R30 (Q9, Q10).** Dialogue isolation is N0.3(g) with R18's definitions: a measured combination that
  **refuses** when the gain is not positive, and §1.3 records that AU5 ships **no separation model**.
  De-click ships **conservative** after R2 and R7, and AU6's real material moves the ceiling
  (§3.5, §5.6).

**Resolved before implementation (N2, on `critic-contract.md` F1–F25 and its rulings on revision
1's three open notes).** Every finding is accepted with the critic's exact replacement text, folded
into the rule, table cell or checklist item it belongs to and never carried as an appendix. R31–R45
are the fifteen findings that change normative text, one register entry each; R46 carries the ten
nits; and R47–R49 are the three open-note rulings, which **close** those notes rather than carrying
them — revision 1's open-note block is deleted, not amended.

- **R31 (F1).** §3.11(c)'s click alternates sign **per frame** — `0.9 × (−1)^(k + j)` for
  `j in 0..8`, with `k` fixing only each click's starting polarity — because a constant 8-frame step
  has `d2 = +c, −c, 0, 0, 0, 0, 0, 0, −c, +c` and is invisible in its middle six frames, which would
  read `click_count == 38` and miss the 30 dB drop. Rule 53's `4 × 0.9 = 3.6` **stands**, with the
  "sign-alternating run" clause beside it (§3.11(c), §3.5 rule 53, A6, A9).
- **R32 (F2).** The OLA delay is pinned **directly**, not through the impulse: a unit test forces
  `g_k ≡ 1.0` for every bin and block and asserts the `window − 1` delay to **1e-12** over the
  fully-overlapped region, with a second arm on `pad == 65 / 18 / 129`. The impulse pins the
  **declared** delay only, because rule 41 puts an all-neutral node on the `direct` branch
  (§3.10 rule 71, A5).
- **R33 (F3).** `third_octave_spectrum` (spectrum.rs:126-205) is **parameterised** by
  `(segment_frames, hop_frames, minimum_frames)`; AU2's callers pass today's constants with their
  goldens pinned byte-unchanged, and the profile passes 4 096 / 2 048 / 22 528, which
  `SPECTRUM_MINIMUM_FRAMES = 24 576` would otherwise **refuse**. `mix_noise_profile` therefore
  differs from `mix_spectrum` in **three** named things, and **eleven** bands (20 Hz … 200 Hz) are
  `window_limited` at a 4 096 window, not five (§3.7 rule 61, §8, A4).
- **R34 (F4).** Rule 43 carries **three conversions, normatively and nowhere else**: a `None` band →
  `PROFILE_BAND_NEUTRAL_TENTH_DB`; hundredths → tenths by `i32::div_euclid(10)`; and a clamp into
  `-1200..=0`. `SILENCE_POWER` (spectrum.rs:36) joins R21's `pub(crate)` promotions, or `export.rs`
  cannot compile (§3.3 rule 43, §8, A4).
- **R35 (F5).** `normalization_context` carries **every `AudioBus` field except `effects`**,
  including **`gain_tenth_db`**, which `normalization_bus` (server.rs:9726-9734) writes as 0 today —
  a field left off the list is a field silently reset. B11 gains a non-zero-fader and `gain_curve`
  arm in both orders (§5.7 rule 111, B11).
- **R36 (F6).** Rule 10 is rewritten to the function's real **parameter-first** match shape: AU5
  makes **two edits**, not three arms — the `AUDIO_LOOKAHEAD_PARAMETER` arm's inner `matches!`
  widens, and one guarded arm is inserted before the `_ => false` wildcard (§2.2 rule 10).
- **R37 (F7).** The **three frame domains** — audio sample, source and project — are spelled once in
  rule 63 with their ceil conversions and per-asset derivation, and `mix_noise_profile` **re-checks**
  the rendered sample-frame count itself rather than trusting the caller's conversion
  (§3.7 rule 63).
- **R38 (F8).** Rule 72 names **two** divergent states, not one: the per-bin gain smoother and rule
  41's `direct_switch_remaining` window. Both are bounded and neither is reachable from export, and
  A14 gains an arm pinning the switch window at exactly `window − 1` frames (§3.10 rule 72, A14).
- **R39 (F9).** Rule 36 is rewritten: the ring, the `bypass_delay` feed and the **block clock** run
  on **every** frame, so the grid stays anchored at project sample 0 and a switch never re-phases it;
  only the transform and the emit differ; and `direct_switch_remaining` **discards** accumulator
  frames rather than emitting them (§3.2 rule 36).
- **R40 (F10).** The `direct` branch **narrows** AU2's bypass rule (audio.rs:941-947) rather than
  following it, and is recorded as an **AU2 §0 erratum beside R6's** (§3.2 rule 41, §1.4, §8).
- **R41 (F11).** `detect_clicks` applies rule 54's **1 ms pre/post `d2` guard** as well as the
  length ceiling, so a `ClickReport` counts exactly the spans the node **would repair** and §3.9's
  density cannot drift from the node (§3.5 rule 57).
- **R42 (F12).** `track_gaps(&self, track: TrackId) -> Option<Vec<Range<TimeCode>>>`, answering
  `None` for an unknown track — the house shape `Document::track_audible` already uses, and the
  spelling every caller already holds (§5.2 rule 93, B3).
- **R43 (F13).** `MixerChainKind` does not exist: the type is core's **`AudioChain`**, which
  `MixerSelection::chain()` already returns, and `Option<(AudioChain, EffectId)>` keeps `MixerFrame`
  `Copy` (§6.3 rule 126).
- **R44 (F14).** The fill clip is written at **`speed_percent = 100`**, which is what makes rule 97's
  helper a true inverse of `clip_duration`; a retimed fill is an ordinary edit afterwards and the
  planner never proposes one (§5.3 rule 97).
- **R45 (F15).** Every fixture call is spelled with the **real** helper signature —
  `tone(frequency, amplitude, rate, frames)` mono and `pseudo_random_amplitude(samples, amplitude)`
  flat, its first argument a **sample** count — with the interleave to stereo stated once; the
  promoted signatures are **unchanged**, because changing them would break `audio.rs`'s existing
  callers (§3.11, §5.4).
- **R46 (F16–F25).** Ten corrections, each folded at its own site: the last click occupies frames
  `91_200..91_208` (§3.11(c)); the multi-tile arm asserts **all three** joins (§5.4 rule 101, B5); a
  profile documentation row is ≈ **49 B**, so the unhatched cost is ≈ **10.3 kB** (R23, §4.2 rule
  78); the printed `rms(corrupt − clean)` differs from the analytic figure by **0.24 dB**, not ~3
  (R20, §3.11(c)); both "six" doc comments (mixer_pane_ui.rs:1602, :1616) are named in §8; the
  drifted citations are corrected (rule 12 → operation.rs:4036-4038, rule 27 → spectrum.rs:43-46,
  rules 16 and 122 → mixer_pane_ui.rs:1623-1625 and :1636-1649); B9 asserts with **`contains`**;
  `capture_room_tone` **overrides** `probe_path`'s name (rule 115); the **caller** filters
  `timeline_silences` on `TimelineSilenceSpan.track` (rules 64 and 127); and the bus-pane row is
  measured with the **hum** card expanded, the tallest of the three (rule 131).
- **R47 (open note 1, `MIXER_BUS_PANE_BUDGET`, ruled).** 520.0 is adopted with its meaning
  corrected: it is a **content-height** budget inside `chain_pane`'s existing `ScrollArea`, so an
  overrun costs scroll distance and **never** clipping, and
  `the_chain_pane_with_an_automation_section_still_fits_the_dock` (mixer_ui.rs:4786-4836) is already
  the fit proof. It is measured with the **hum card** expanded; `MIXER_NOISE_WELL_HEIGHT` 48 → 32 is
  the first cut and buys 16 px of scroll rather than rescuing a hidden control; `MEASURED` lands as a
  §0 erratum (§6.5 rule 131).
- **R48 (open note 2, `ROOM_TONE_MAX_FILE_BYTES`, ruled).** 32 MiB, the 60 s writer cap and the
  500 ms floor ship **unchanged**, and rule 90 gains the sentence recording **why** the two caps
  differ by 9 MB — the file cap guards the **reader** against a file nobody wrote, the millisecond
  cap guards the **writer** — so a later reader does not simplify them into one number (§5.1 rule
  90). No erratum.
- **R49 (open note 3, the `direct`-branch switch rule, ruled).** The `direct` branch and its
  `window − 1` switch window are **adopted**, as amended by R38, R39 and R40, and folded into §3.2 as
  **normative** text rather than an erratum: this is a state machine, not a measurement. The
  `511 ≥ window − hop = 384` argument stays in rule 41 (§3.2 rules 36, 41, 72).

**Implementation errata, Part A core (2026-09-10):**

- **R50. `sort_exceptions` is shared, not re-spelled.** Rule 24 says `audio_repair_exceptions`
  uses the **same** sort order as `audio_qc_exceptions`, `(severity desc, code asc, field asc)`.
  The comparator behind that order was a private `fn sort_exceptions` in `audio_qc.rs`; it is
  now `pub(crate)` and `audio_repair.rs` calls it, so the two modules cannot drift apart on
  "same order". §8's Part A core file list therefore also touches `audio_qc.rs`, by one word.
  No behaviour changes and `audio_qc_exceptions`' output is byte-identical.
- **R51. The 31 profile rows are generated from one name table, not 31 literals.** §2.1's table
  spells `profile_band01_tenth_db … profile_band31_tenth_db` as a range. `effect.rs` carries
  `NOISE_PROFILE_PARAMETER_NAMES: [&str; 31]`, `PROFILE_BAND_NEUTRAL_TENTH_DB` and
  `NOISE_PROFILE_BAND_COUNT` as `pub` items re-exported from `lib.rs`, and a `const fn`
  assembles the descriptor's 36 rows from them — the shape `with_matte_parameters`
  (effect.rs:995) already uses for CC5's 47 matte rows. Rule 7's `is_noise_profile_parameter`
  is still the predicate every reader outside `effect.rs` uses; the name table exists so the
  descriptor itself cannot disagree with the predicate about *which* 31 names those are, and
  so §2.3's QA rule and the app's `insert_audio_effect` can enumerate them without re-deriving
  the spelling. `AUDIO_DENOISE_PARAMETERS` stays private.
- **R52. AU2's descriptor-count and audio-name pins are prefix pins now.**
  `tests/au2_core.rs`'s A1 test asserted `EFFECT_DESCRIPTORS.len() == 25` and that the
  registry's audio names were **exactly** AU2's eight, in order. Both are amended in Part A:
  the count reads **28**, and the name assertion becomes a prefix assertion — AU2's eight come
  first, in AU2's order — with AU5's own eleven-name and 28-entry pins living in
  `tests/au5_core.rs` per A1. The `tail` arm that read the last three entries now reads the
  three **after `audio_limiter`** by name, so appending to the registry cannot silently pass it.
- **R53. `noise_profile_missing` reads the whole reduction curve.** Rule 18 says the warning
  fires when `reduction_tenth_db` "resolves above 0". `qa_document` has no project frame to
  resolve at, so it reads the parameter's whole range through `qa.rs`'s existing
  `parameter_range` helper — the stored value, plus every keyframe value — and fires when the
  **maximum** is above 0. A node parked at 0 that rides up under a curve therefore earns the
  warning, which is the reading rule 18's "wasted node" wants. The message is rule 18's,
  verbatim, with `{r}` the same maximum.

**Implementation errata, Part A media (2026-09-10).** R60–R79 is the media crate's reserved range.

- **R60. `engine.rs` joins §8's Part A media file list.** §8 names the three `Analysis` methods
  under **core** `media.rs` and lists no media file that overrides them, but a
  `NotImplemented` default compiles silently — the same hazard rule 86 records for the app's
  proxy. `FfmpegMediaEngine`'s `impl Analysis` (engine.rs:758) is the only one in the crate, so
  it gains three one-line arms forwarding to `export.rs`'s three `measure_*` functions. One word
  each; no behaviour that §3.7–§3.9 does not already specify.
- **R61. `output_pad` is fed on every frame, not only on the OLA branch.** Rule 36 says the
  `direct` branch "pops and **discards** one frame from the accumulator". Taken literally the pad
  is then starved while `direct` is set, and when rule 41's switch window closes the pad emits
  `latency − (window − 1)` frames of stale or zeroed material — a dip exactly where rule 41
  promises there can never be one. The popped frame is therefore *discarded from the output* but
  still pushed through `output_pad`, so the pad holds real reconstruction by the time the window
  closes. The arithmetic that makes the window long enough is `window − hop` frames to reach full
  overlap **plus** `pad` frames to fill the line: `384 + 65 = 449 ≤ 511` at 48 kHz,
  `384 + 18 = 402 ≤ 511` at 44.1 kHz and `768 + 129 = 897 ≤ 1023` at 96 kHz. Rule 41's
  `511 ≥ window − hop = 384` argument stands; it was simply not the whole sum.
- **R62. Four lanes live in `src/`, not in `tests/au5_fixtures.rs`.** §7 attributes §3.11(c), A5's
  `to_bits()` transform pin, `AU5_PREROLL` and rule 43/44's direct conversion tests to the fixture
  file, but each needs an item the fixture file structurally cannot see: an integration test links
  the crate's **public** surface, and `process_buffer_static`, `forward_fft`, the pre-AU5
  reference transform, `denoise_bin_floor_power` and `profile_band_tenth_db` are all
  `pub(crate)`. Rather than widen the public API for a test, §3.11(c) and rule 44's conversion pin
  live in `audio.rs`'s test module, A5's bit-identity, the round trip and `AU5_PREROLL` in
  `spectrum.rs`'s, and rule 43's three conversions in `export.rs`'s. `tests/au5_fixtures.rs`
  carries every lane the public `Analysis` surface supports — (a), (a′), (b), (d) and A10's
  hum-separation arm — in §5.8's house style, with the same fixture bytes and the same promoted
  helpers. `AU5_PREROLL` additionally prints `profile=debug|release`, because the table replaces a
  `sin_cos` per butterfly and an unoptimised build spends its time elsewhere.
- **R63. The de-click line is a private ring in `audio.rs`, not `dsp::DelayLine`.** Rule 56 says a
  repair "writes into the delay line's buffer at the span's position". `dsp::DelayLine` is
  deliberately opaque — one `process(frame)` that swaps, no buffer accessor — and `dsp.rs` is not
  on Part A's file list, so `DeclickLine` is spelled in `audio.rs` with the same semantics plus
  the positional `get`/`set` a repair needs. `audio_denoise`'s two lines are still
  `dsp::DelayLine`.
- **R64. `HUM_GOERTZEL_BLOCK_FRAMES` is 24 000, not 4 800.** R18 fixes the floor at "**≥ 4 800**"
  and §3.9 spells 4 800 with the rationale "enough to resolve 50 from 60 Hz", which it is. But the
  *shoulders* the excess is measured against sit at `f × 2^(±1/6)` — only 5.45 and 6.12 Hz from a
  50 Hz fundamental. Over 4 800 rectangular samples those are 0.545 and 0.612 bins away, where a
  pure 50 Hz tone still reads −4.8 and −6.2 dB, so the largest excess any hum can show is about
  5 dB: under `REPAIR_HUM_EXCESS_HUNDREDTHS` (600), and `mains_hum_present` could never fire. At
  24 000 frames — **500 ms at 48 kHz**, and correspondingly 544 ms at 44.1 kHz or 250 ms at 96 kHz,
  because the constant is a **frame** count and not a duration — the same shoulders read −21 and
  −34 dB, and the measured fixture reads a 27.5 dB excess at 50 Hz against 0.0 at 60. The constant
  keeps its name, its units and R18's floor. **The cost, recorded (pass-1 finding 10):** the range at
  which a hum measurement stops being fabricated and starts being honest moves from 100 ms to
  **500 ms at the 48 kHz measurement rate** (`export.rs`'s `AUDIO_RATE`, which is the only rate
  `measure_audio_repair` ever runs at). A
  range between the two now answers `None` for both mains frequencies and both harmonic vectors,
  which is §3.9's "Both are `None` under one whole block" exactly — never a zero. `AudioRepairReport`
  carries no field naming the block it was built with, so a consumer cannot distinguish "no hum"
  from "range under one block"; the `None` is the signal, and
  `au5_the_repair_inspector_refuses_to_invent_a_percentile` drives that arm. The same report is
  silent about the click ceiling R65 uses; both belong in `get_audio_repair`'s prose, agent side,
  beside R18's percentile-bias sentence.
- **R65. The inspector's click detector reads the descriptor's `max_click_milliseconds`
  *maximum*.** §3.9 says `detect_clicks` runs "at the descriptor's neutral threshold", which fixes
  `detector_threshold_tenth_db` at 240 and says nothing about the span ceiling. The neutral
  ceiling is **0**, at which no span is short enough to qualify (§2.1) — so a report built at the
  neutral would read `click_count == 0` on every recording ever made. The inspector therefore
  passes the descriptor **maximum**, 1 ms: it counts the spans a node *could* repair, which is
  what rule 57's "exactly the spans the node would repair" means for a measurement taken before
  any node exists.
- **R66. Rule 56's Catmull-Rom is the non-uniform Hermite form.** The four control points are the
  two good samples either side of the span, so `p0`–`p1` and `p2`–`p3` are **one sample** apart
  while `p1`–`p2` is `span` samples apart. The uniform Catmull-Rom tangent `(p2 − p0)/2` conflates
  the two scales and understates the end slopes by roughly `span/2`; on §3.11(c)'s fixture it left
  a residual of 1e-2 rather than the 1e-4 the `A(ωH)⁴/384` bound predicts, and the lane measured
  42.3 dB against a 30 dB budget — a 1.4× margin. With `m1 = (p1 − p0)·span` and
  `m2 = (p3 − p2)·span` the same spline measures **69.7 dB**, margin 2.32×. Rule 56's choice of
  Catmull-Rom over linear, and its stated reason, are unchanged.
- **R67 (rewritten after pass-1 review; the first text is withdrawn). §3.11(a)'s tail window was
  measuring a transient, not the gate.** The withdrawn text claimed the drop was capped near 12 dB
  "by construction" and independent of `reduction_tenth_db`, and halved the budget on that premise.
  Both claims are false and the reviewer's independent replica of rule 37's whole pipeline showed
  why. **(i) The `E[g]` figures were wrong.** Integrating
  `E[g] = g_floor·P(r < a/(1 − g_floor)) + ∫(1 − a/r)·2r·e^(−r²)dr` numerically at `a = 1` gives
  **0.1561 → −16.13 dB** at `reduction_tenth_db = 200` and **0.0954 → −20.41 dB** at 400, not
  0.259 / −11.7 and −12.7. The 0.259 was fitted to the measurement, not derived. **(ii) The gate *is*
  reduction-dependent**, exactly as those figures say. **(iii) The 4 dB the lane was short is a
  measurement-window artefact**: `secs(2.5..3.0)` begins on the *exact sample* the 1.5 s 1 kHz tone
  ends, so its first ~110 ms is rule 37's 50 ms smoother releasing from the tone — 7.15 dB over that
  stretch against 15.98 dB over the last 250 ms. The fix is one line of **fixture**, not a constant:
  the tail is measured from **2.6 s**. Nothing in the shipped DSP changed for this erratum; the
  replica reproduced the old 11.98 dB to 0.06 dB, so there was no bug, only a lane pinning the wrong
  thing and an erratum explaining it wrongly.

  `DENOISE_FLOOR_DROP_BUDGET_TENTH_DB` therefore moves **90 → 70**, not 90 → 50, and for a
  different reason: R75 corrects rule 44's `sum w^2` from the sine window's `N/2` to a periodic
  Hann's `3N/8`, which lowers every bin floor by 1.25 dB. **That is not a decibel subtraction from
  `E[g]`** — pass-2 caught the first rewrite doing exactly that, and −16.13 ± 1.25 is neither
  −14.4 nor anything else the lane reads. The integral has to be **re-evaluated** with the scaled
  floor: `a = 10^(−1.25/20) = 0.866` gives `E[g] = 0.1901 →` **−14.42 dB** at reduction 200 (and
  `0.1390 → −17.14 dB` at 400), against this lane's measured **14.41 dB** — 0.01 dB between a
  first-principles Rayleigh integral and the shipped pipeline, which is the strongest analytic
  agreement in the AU5 evidence set and much better evidence than the budget move itself.
  **Why `a = 1` described the pre-R75 lane, once:** rule 44's conversion was 1.25 dB high and §3.7
  rule 61(ii)'s 20th-percentile learn reads about 1.25 dB low, so the two cancelled and the
  subtracted floor happened to equal the true one. R75 removed the conversion error and left the
  percentile bias, which is why `a` is now 0.866 and why R75 is a real correction rather than a
  re-tune. Margin **2.06×** — 90 would have left 1.60×, under the house floor, which is why 70 and
  not the contract's own number.

  **The 0.41 dB of headroom above the 2× gate is deliberate** (pass-2 finding 7). The assert trips
  at a measured 14.0 dB, thinner than any other AU5 lane (hum 2.20, de-click 2.32, SNR 2.44, learn
  7.50), because the budget is pinned to an analytic steady state rather than padded around a
  measurement: anything that shallows the gate by half a decibel is a **red lane rather than a
  silent drift**. Three changes would move it — a further correction to rule 44's conversion, a
  different percentile in rule 61(ii), or a smoother long enough to keep the tone's release inside
  the 2.6 s tail window — and each of those is a thing a reader should be made to look at. Dropping
  the budget to 65 would buy 2.22× and give that up. Nothing in the lane is OS-dependent: the
  fixture is exact `wav_f32` bytes through a lossless f32 PCM decode, `pseudo_random_amplitude` is a
  fixed xorshift64, and the transform and Welch estimate are this crate's own `f64` code.

  `DENOISE_TONE_LOSS_BUDGET_TENTH_DB` stays 10 against a measured 0.03 dB, margin 33×.
  `AUDIO_REPAIR_SNR_GAIN_BUDGET_HUNDREDTHS` stays **600** against a measured **1 461 hundredths**,
  margin 2.44×: the 10th percentile of 10 ms windows falls *slightly* further than the 400 ms
  mean-level drop the tail lane reads — 14.61 dB against 14.41, **0.20 dB apart**, not the ≈ 4.5 dB
  the first rewrite's stale "≈ 11.7 dB" clause implied — because the residual after spectral
  subtraction is fluctuating rather than stationary. The two lanes measure different ranges (this
  one `range: None` over the whole 3 s fixture, tone third included) and neither may be re-baselined
  from the other's evidence. §3.11(a′)'s gate expectation is a separate matter and is R78.
- **R68. §3.11(b)'s 300 Hz loss is ≈ 0.56 dB, not 0.13.** R20 corrected the brief's 0.05 dB by
  summing per-section estimates; the design's own analytic response — `hum_removal_magnitude_db`
  at 300 Hz, which the lane's analytic arm matches to **0.026 dB** — is 0.56 dB, because an RBJ
  peaking section's skirt at Q 12 falls more slowly than the estimate assumed.
  `HUM_TONE_LOSS_BUDGET_TENTH_DB` moves **5 → 15** to keep the ≥ 2× margin (2.68×) against what
  the design really does. The analytic arm is what proves the number is the design and not a bug;
  `HUM_DROP_BUDGET_TENTH_DB` is unchanged at 140 against a measured **30.7 dB**, margin 2.20×.
- **R69. §3.11(a′)'s neighbouring-band rule is strengthened, not just widened.** "Every
  neighbouring band moves under 3.0 dB" is unreachable on a fixture the profile is learned from:
  a neighbour's measured content is Hann leakage from the band itself, about 25 dB down, and it is
  *part of the learned floor*, so the gate legitimately takes it down too — by 5.6 dB. The lane
  pins the statement that actually makes this a band gate: every neighbour carrying anything above
  the `SILENCE_POWER` floor moves **strictly less than the learned band does**, under a 7.0 dB
  ceiling, with at least two such neighbours checked. Bands reading the silence floor are excluded
  because "nothing" cannot move by a ratio.
- **R70. §3.11(a′)'s 64 partials carry Schroeder phases.** "Phase-randomised" is not enough: any
  phase set that is **linear** in the partial index across equally spaced frequencies is a pure
  *delay*, so the 64 partials realign into a pulse train every `1/3.6 Hz = 278 ms`. A 4 096-frame
  profile window then either catches a burst or misses it, the per-window band level swings 25 dB,
  and the 20th-percentile reduction reads **22 dB under** the analytic level — the lane fails
  while both the fixture and the code are correct. The quadratic ramp `φ_p = π p²/P` makes the sum
  a constant-envelope chirp across the band; every window carries the same power and the profile
  reads −40.2 dB against an analytic −40.0.
- **R71 (rewritten after pass-1 review; the first text recorded a defect instead of fixing it).
  `reduction_tenth_db`, `floor_offset_tenth_db` and `smoothing_milliseconds` are read per project
  frame.** The withdrawn text put all three on rule 41's construction-and-epoch cadence and recorded
  the consequence as an AU2 §0 E16-style divergence. That was the wrong call twice over. It is not
  a *divergence* at all — `static_audio_value` reads the stored value in **every** path, so playback
  and export agreed bit for bit and no `assert_playback_matches_export` arm could ever have caught
  it — and what it actually did was worse: a keyframe curve on any of the three was **accepted by
  validation and then silently discarded forever, in both paths**. §2.2 rule 10 enumerates the
  static predicate's members as exactly the five lookahead pairs plus the 31 profile rows, and
  §2.1's table marks these three with nothing, so they are ordinary keyable parameters; rule 11
  refuses precisely this trade for `max_click_milliseconds`, a *lesser* control than the reduction
  an editor rides while listening.

  They are now read through the `(project frame, parameter epoch)` cache the node already keeps for
  `bypass` — the `audio_declick` arm is the idiom — and rule 41's `reduction_tenth_db == 0` term is
  derived from **that same read**, so the predicate and the gain law it guards can never be of
  different vintages. The all-neutral-profile term stays on the epoch cadence, because only the
  bands force the bin floor table to be rebuilt (rule 73), which is what rule 41 actually needs.
  The parameter staircase runs at the **project-frame rate, which is coarser than the hop, not
  finer**: at 30 fps and 48 kHz a project frame is 1 600 sample frames against `hop = 128`, so
  12.5 blocks inside one project frame read the same cached value, and the cache costs one map probe
  per project frame rather than one per sample. (The first rewrite called this "a block-boundary read
  spelled on a per-project-frame cache", which reads as though the value could change every 128
  samples; it cannot, and the cadence is AU4's automation cadence everywhere else.)
  `au5_a_keyed_reduction_curve_is_heard` drives a `Hold` curve stepping 0 → 400 at project frame 5
  and asserts **sample-wise** that the node is an identity before it — not an RMS comparison, which
  any energy-preserving all-pass would satisfy — and that it gates by more than 6 dB after; it fails
  on the withdrawn behaviour, because the fixture's *stored* reduction is 0 and
  `static_integer_parameter` ignores keyframes, so the old read put the node on the `direct` branch
  for the whole buffer.

  **Both paths are pinned on a keyed chain, not asserted from the structure** (pass-2 finding 3).
  The first rewrite closed with "both paths still match at 1e-6 (`au5_playback_matches_export_on_a_
  repair_bearing_chain`), because both evaluate the same curve at the same project frame" — but that
  fixture carried **no curve at all**, so the arm pinned parity of the static read, which was never
  in doubt. `parity_document_with_repair_chain` now carries a `Hold` curve on `reduction_tenth_db`
  stepping 0 → 200 at **project frame 10**, and both of `assert_playback_matches_export`'s arms —
  chunked playback and the frame-5 seek — cross it inside an asserted window. Frame 10, not the
  18..20 window pass-2 suggested: that bus carries only track 2, whose one clip spans project frames
  4..14, so the bus stem is silent after 14 and a step there would change nothing (the first attempt
  measured a difference of exactly 0 and said so). Frame 10 sits inside "trimmed source" (6..14),
  five frames past the seek target, and leaves 19 200 sample frames of asserted material after
  rule 41's 511-frame switch window closes. Crossing the step is a `direct` **clear**, so what the
  arm now pins at 1e-6 is the keyed read, that transition and the switch window, in both paths. A
  second assertion keeps the curve load-bearing: the same document with `keyframes` cleared exports a
  measurably different mix inside the asserted window (max difference **0.133**), so a refactor that
  drops the curve fails rather than silently reverting the arm to the static case. The structural
  reason it holds is worth recording too: both drive loops derive `project_at` from
  `start_sample + frame`, so a curve step lands on the same absolute sample and therefore the same
  OLA block in both paths, and `needs_seek_preroll` forces `decode_from = TimeCode::ZERO`, so
  `since_block` is anchored at project sample 0 even on the seek arm.
- **R72. Rule 53's arithmetic is right about the mechanism and wrong about the count.** "Without
  the exclusion the detector is self-masking … the node would detect **nothing**" holds for a
  window that *contains* the click. The window rule 53 specifies is **strictly causal**, ending at
  `n − 1`, so the click's own leading edge trips before the click enters the reference: the naive
  detector flags **114** frames of §3.11(c)'s fixture — 6 of each click's 8 — and then masks the
  rest. The exclusion is still the whole detector, and the test proves it decisively rather than
  by the count: given a **perfect** repair of exactly the frames it flagged, which no interpolator
  could deliver, the naive reference still drops the error by only **5.98 dB** against the
  specified detector's 69.7 and the lane's 30 dB budget.
- **R73. `HumState` holds `Vec<BiquadSection>`, not `Vec<[BiquadSection; HUM_SECTIONS]>` per
  channel.** Rule 48 spells the state as an array of sections per channel, but `BiquadSection`
  already owns its own per-channel memory (dsp.rs:168-215) — the `audio_parametric_eq` idiom rule
  48 itself cites. Ten sections, each `channels` wide, is the same allocation with one fewer
  dimension and the same "allocated at construction from the descriptor maximum" property.
- **R74. AU2's `every_audio_effect_kind_changes_a_tone` sweep gains a per-node input.** A13's
  assertion is that every `is_audio_effect` name changes a tone at a non-neutral setting, driven
  from one 1 kHz tone. Two AU5 nodes cannot be exercised by it and are right not to be: a de-click
  node is an identity on material with no click in it *by construction* (rule 56), and a 50 Hz hum
  cascade has no section within an octave of 1 kHz. The sweep now asks for the input each node's
  own contract describes — the same tone with one 8-frame click for `audio_declick`, a 100 Hz tone
  for `audio_hum_removal` — and compares against **that** buffer, so the assertion is still the
  one `process_frame`'s fall-through arm cannot pass by accident. The bypass sweep is untouched
  and still runs every node on one shared pseudo-random buffer.
- **R75. Rule 44's `window² / 4` is the sine window's constant; a periodic Hann's is `3N/8`, and
  the sum is now read from the window.** Rule 44's derivation says "a periodic Hann has
  `Σ_n w_n² = N/2`". It does not: `Σ_{n<512} (0.5(1 − cos 2πn/512))² = 192 = 3·512/8` exactly, and
  `N/2` is the **sine** window's figure. Every bin floor was therefore `4/3` too large — **+1.25 dB
  of over-subtraction at every frequency** — and the rule's own "sanity check" could not see it,
  because both sides of the check used the same wrong constant. The closed form is replaced by
  `bin_floor_power[k] = 10^(L_k/100) · 0.5 / bins_in_band · (Σ_n w_n²) · window / 2`, with `Σw²`
  computed from the very `hann_window` the analysis uses, exactly as `third_octave_spectrum` already
  computes its own `window_power` — so no constant can be wrong again and the runtime window and the
  measurement window cannot drift apart. The general form is the honest one:
  `E|X_k|² = (P_band / bins_in_band) · (N/2) · Σw²` follows from Parseval with no window assumption
  at all. Consequence: the gate is 1.25 dB shallower everywhere, which is half of why R67's budget
  lands at 70 rather than 90. The rule 44 test now asserts the **bias** of the conversion against a
  real Hann-windowed `forward_fft` of white noise of known mean square (+0.023 dB at a 512-point
  window, +0.003 at 1 024, printed as `AU5_BIN_FLOOR`) instead of asserting the floors are finite,
  which is what let a 1.25 dB error live in the one factor the derivation paragraph exists to
  justify. The mean, not the worst: `bins_in_band` is an integer count standing in for a continuous
  bandwidth and carries up to 1.4 dB of scattered, zero-mean quantisation, while a wrong `Σw²` is a
  bias on every bin at once.
- **R76. The Nyquist bin was gained twice.** `bins = window/2 + 1`, so rule 37's loop reaches
  `bin == window/2`, whose mirror `window − bin` is **itself**; the mirror multiply then applied
  `g²` to that one bin. It cannot break conjugate symmetry — the Nyquist bin is real — and the
  `g ≡ 1` OLA pin cannot see it, because `1² = 1`. The guard gains `&& mirror != bin`.
- **R77. A5's `bypass` toggle is driven through the epoch path, not through
  `process_buffer_static`.** A5 asks for a test that "toggles `bypass` mid-buffer through
  `process_buffer_static`". That is impossible as written: the helper pins
  `project_at = TimeCode::ZERO` on every frame, so a `Hold` curve on `bypass` cannot change value
  inside one call. The toggle is driven instead through rule 73's **retarget** path, retuning
  `reduction_tenth_db` 200 → 0 → 200 mid-buffer, which is a real editor gesture and exercises the
  identical `direct` predicate. Two further corrections to how the arm is written, both of which the
  first version got wrong and neither of which is cosmetic: the signal is **noise, not a tone**, and
  the assertion is **sample-accurate against the delayed input, not an RMS floor** — a starved
  `output_pad` (the bug R61 records) emits frames from before the switch, which on a steady tone
  have the right level and the wrong phase, so an RMS floor cannot see them. With the fix the arm
  reads a worst error of 3e-7; reintroducing R61's bug makes it read **0.45 at frame 24 532**, which
  is `on_at + 511 + 21` — the pad's stale frames, exactly where R61 says they land. The per-block
  no-dip assertion rule 41 actually spells is kept alongside.
- **R78. §3.11(a′)'s gate term is a derived bracket, not a centre and a budget.** The first version
  of this lane carried an "expected" of −9 dB whose own stated arithmetic gives −10.67 dB
  (`20·log10(1 − 1/√2)`), i.e. a number fitted to the measurement wearing a derivation. The
  arithmetic is now correct **and** the shape of the assertion changes, because a centre this
  fixture cannot predict to ±3 dB is not worth asserting against. Rule 37's clamp never binds on
  material whose bins sit above the spread floor, so `reduction_tenth_db` does not enter; the
  two-bin concentration model gives a **bound**, `g ≥ 1 − 1/√bins_in_band = 0.2929` → **−10.67 dB**,
  which the residual may not be deeper than without the profile over-subtracting — precisely the
  failure R75 removed 1.25 dB of. The lane asserts that bound and, as its other half, that the gate
  removes at least 3 dB, so it fails on both real regressions; the printed `margin` is the **learn**
  margin (7.5×), which is the term that does have an analytic centre, and both gate clearances are
  printed beside it (3.3 dB deep, 4.35 dB shallow, measured −7.35 dB).
- **R79. Where the fixed pass-1 findings are pinned.** Three arms exist only because the first pass
  had no way to fail: `au5_the_bin_floor_conversion_holds_its_derivation`'s bias arm (R75),
  `au5_a_keyed_reduction_curve_is_heard` (R71) and the sample-accurate half of
  `au5_the_direct_switch_window_is_window_minus_one_and_never_dips` (R61/R77). Two more close
  named gaps in §7 rather than in the code: `au5_the_repair_inspector_refuses_to_invent_a_percentile`
  drives A10's "`None` below ten energetic windows with a `low_window_count` finding" and R64's
  500 ms honest-`None` floor in one range, and `au5_every_repair_neutral_is_a_bit_exact_identity`
  gains a **configured** bypass arm — a learned, reducing denoiser and a configured de-click at
  `bypass = 1` — because an all-neutral node is on the `direct` branch for the structural reason as
  well, so it never puts `bypass_delay` under test at all. `AU5_REPAIR_SHA256` is left as AU3's
  print-only idiom (pass-1 finding 9) with a doc comment saying plainly that a render compared
  against itself proves determinism and that what discharges A12's pre-AU5 identity claim is the
  610 unchanged green tests around it.

  **Pass-2 additions, folded rather than numbered** (R60–R79 is full): the keyed both-paths arm on
  `parity_document_with_repair_chain` and its load-bearing check are recorded in R71; the
  re-evaluated `E[g](a = 0.866) = 0.1901 → −14.42 dB`, the `a = 1` cancellation that described the
  pre-R75 lane, the corrected `AUDIO_REPAIR_SNR_GAIN_BUDGET_HUNDREDTHS` justification and the
  deliberate 0.41 dB of headroom are recorded in R67; R71 also carries the corrected project-frame
  cadence sentence and the sample-wise identity arm; and R64 carries the rate-relative reading of
  `HUM_GOERTZEL_BLOCK_FRAMES`. Two dead lines went with them: an unused `fundamental_hertz` insert
  on a de-click effect at the end of the identity test, and the RMS form of the keyed test's
  identity arm.

**Implementation errata, Part A agent (2026-09-10).** R80–R89 is the agent crate's reserved range,
appended here for the same reason the app's block above is: three implementers editing §0 at once
cannot collide if each owns a range and one anchor line.

- **R80. `plan_clip_fades`' window is the fade itself, and its published evidence is an RMS
  level, not a true peak.** Rule 67 says the two-render head/tail hack "becomes one
  `mix_window_levels` call" and that the sub-gating-block skip arm "goes away", but does not say
  what window to ask for. The planner asks for `fade_milliseconds.clamp(1, 1000)` — the audio the
  proposed ramp would actually act on — hop equal to window, one call per requested **track**
  (`mix_window_levels` takes one `MixSpectrumPoint`, so one call per track is the minimum, and a
  hundred-clip timeline that cost 200 renders now costs one per track). Three consequences are
  recorded rather than left to a reader to discover. (i) The published per-clip evidence changes
  name and meaning: `head_true_peak_dbtp_hundredths` / `tail_true_peak_dbtp_hundredths` become
  `head_dbfs_hundredths` / `tail_dbfs_hundredths`, because §3.8's accessor reports a short-window
  **RMS level in dBFS** and AU4 read a true peak in dBTP. `threshold_dbfs_hundredths` was always
  spelled dBFS and is now honestly compared against one; the default −4 000 is 40 dB down, far
  under any real head or tail, so no fixture's proposal moves — A17's real-media regression
  (`au4_plan_clip_fades_measures_the_real_mix_and_commits_set_clip_audio`) proposes the same
  1-frame fade-out and keeps the same editor-set 7-frame fade-in, which is the whole point of that
  test. (ii) `window_sample_frames` and `window_project_frames` keep their names and change value
  — 960 and 1 at the 20 ms default, not 19 200 and 12 — and a `window_milliseconds` key joins
  them, because the window is now a caller-visible consequence of `fade_milliseconds`. (iii) The
  short-clip skip does not disappear entirely: it survives for a clip holding **no whole window**,
  with the reason re-spelled in milliseconds. At 30 fps a 20 ms window can never trip it, because
  one frame is 33 ms; the in-crate arm that proves the branch therefore uses a 200 ms fade and a
  4-frame clip. AU4's three B12 in-crate tests are amended accordingly and the first is renamed
  `au4_plan_clip_fades_proposes_fades_and_measures_in_one_pass`, since "skips short clips" is no
  longer what it proves; the zero-fade test gains an arm asserting that **nothing at all** is
  measured, which the per-track pass makes worth saying. `measure_track_true_peak` is deleted with
  its last caller. `plan_clip_fades`' own description is rewritten to describe the new window **and
  to spell the surviving skip with this erratum's own predicate** — "skipping any clip that holds no
  whole window", not AU4's "shorter than that window" — because the description is the surface an
  agent reads through `get_capability`, and a doc comment, a runtime reason string and a published
  description disagreeing about one predicate is exactly the drift (iii) exists to remove. It costs
  **20 B** (845 → 865, inside rule 135's budget), which is the third line of R81's split.
  **Owed:** `docs/AU4-CLIP-ENVELOPES-AND-AUTOMATION.md` §6.2 still describes the two-render
  measurement and its 400 ms window, and rules 131 and 132 there are now read through this
  erratum. AU4's §0 wants a one-line pointer erratum, exactly as AU2's §0 carries R6's and R40's;
  it is not written here because `AU4-CLIP-ENVELOPES-AND-AUTOMATION.md` is outside §8's Part A file
  list and outside the agent implementer's reserved files.
- **R81. The Part A registry quad, measured (rule 83).** `served_surface_is_small_and_keeps_the_
  internal_registry_discoverable` reports **135 tools, 1 531 264 B serialized = 1 391 430 B of
  input schemas + 117 683 B of descriptions**, against AU4 Part B's 134 / 1 524 370 / 1 389 434 /
  112 948. The served quad is **`(7, 5 660, 3 510, 998)`, byte-identical** for the eleventh
  consecutive measurement, as R22 said it would be. The +1 996 B of input schemas is
  `AudioRepairArgs`' own schema entire: AU5 adds no `Operation` variant, and rule 82's claim that a
  new **descriptor** costs zero input-schema bytes is confirmed — 45 new parameter rows moved that
  column by nothing. The +4 735 B of descriptions splits exactly three ways and sums: **925 B** of
  `get_audio_repair`'s prose, **3 790 B** of `effect_documentation()` growth at 758 B on each of the
  five spliced effect tools, and **20 B** on `plan_clip_fades` (R80). Serialized, 3 084 B is the new
  tool whole, so 3 084 + 3 790 + 20 = 6 894; 1 996 + 925 = 2 921 B is the tool's
  schema-plus-description share of its 3 084 B and the remaining 163 B are its name, annotations
  and JSON envelope — 4 B over `get_audio_qc`'s 159 B, which is exactly how much longer its name is.
- **R82. The hatch, measured against R23's estimate (rules 78, 23, 46).** An enumerated profile row
  measures **48 B**, not 49 (`profile_band01_tenth_db=-1200..=0, neutral -1200`), and 31 of them
  with their separators are **1 550 B**. The pattern sentence with its separator is **175 B**, so
  the hatched growth is **758 B** per spliced tool against an unhatched **2 133 B** — a saving of
  **1 375 B** per tool and **6 875 B** over the five, against R23's estimated ≈ 2 065 B, ≈ 10.3 kB
  and ≈ 6.3 kB. The measured unhatched cost is ≈ **10.7 kB**, so the hatch argument holds with
  slightly more margin than the contract claimed and none of its conclusions move. Rule 79's render
  figures needed no correction at all: the compact block measures **170 B** and the enumeration it
  replaces **898 B**, exactly as R23 predicted.
- **R83. Where the `noise_profile` block sits, and what renders its integers.** Rule 79 does not
  say where in `id:name(…)` the block goes. It is emitted **after** the node's other parameters and
  before the `; keyframes=` suffix, rather than in the `BTreeMap`'s alphabetical position between
  `lookahead_milliseconds` and `reduction_tenth_db`: one block at a fixed place reads better than
  one wedged into an alphabetical run, and the goldens pin it. Rule 79's "its own arm rather than a
  `render_param` call" is honoured for the **block** — nothing routes a joined string through
  `render_param`, which would quote it — while each band's own integer still goes through
  `render_param`, which renders an integer bare. That is deliberate: it means a band somehow
  holding a non-integer is still published, quoted, instead of being silently dropped by the
  parameter loop that now filters profile rows out.
- **R84. The pattern sentence names a Part B capability.** Rule 78's quoted sentence ends "learn
  them with `plan_dialogue_repair`", and it ships in Part A verbatim, on five generated tool
  descriptions, three commits before `plan_dialogue_repair` exists. The alternative — a different
  sentence in Part A and a rewrite in Part B — costs two measurements of the same ledger and leaves
  the intervening surface pointing at nothing anyway. An agent that reaches for the named planner
  in the window between the two commits gets `unknown Kinewright capabilities: plan_dialogue_repair`
  from `get_capability`, which is a clear answer rather than a wrong one. **Part B must not change
  this sentence**, or it will pay a description-byte re-measurement for nothing.
- **R85. `render_mix_spectrum_point` is shared, not re-spelled.** `get_audio_repair`'s rendered
  header names its mix point, and AU2's spelling (`master`, `track {id}`, `bus {id}`) already
  exists as a private `fn` in `server.rs`. It becomes `pub(crate)` and `audio_repair_tool.rs` calls
  it, on R50's reasoning about `sort_exceptions`: two spellings of one point is a drift the type
  system cannot catch. No behaviour changes.
- **R86. What the AU3 sine fixture actually reads, and what A11's lane may not assume.** The
  §7 A15 lane measures `get_audio_repair` on AU3's 440 Hz fixture and asserts the SNR's
  **direction**, not a number, per rule 21. Two fixture facts are recorded because they will bite
  the next reader. (i) A 2 s programme holds 200 whole 10 ms windows but reads **199 energetic**
  ones — one edge window falls under `SILENCE_POWER` — so the lane asserts `190..=200` rather than
  200. (ii) `detect_clicks` at the descriptor's neutral threshold flags **one** span on the
  fixture's onset transient, giving `click_density_per_minute == 30`, which sits exactly **on**
  `REPAIR_CLICK_DENSITY_PER_MINUTE` and therefore raises no finding — the threshold is `>`, not
  `>=`. The lane asserts the density is derived from the count rather than asserting either is
  zero. The measured SNR is 43 hundredths of a dB, which raises `low_signal_to_noise` and is
  exactly rule 21's bias on material with no silence in it: the fixture is the honest worst case
  for a percentile floor, which is why it is the one the lane uses.
- **R87. The fade planner's window indices are media's arithmetic, and they are pinned by index,
  not by level (pass-1 review).** Two amendments to R80's implementation, recorded because both are
  reuse-and-evidence rules the rest of the slice is held to. (i) `clip_window_levels` converts
  project frames to sample frames with **`kinewright_media::frame_to_samples`**, converting each
  absolute bound and subtracting the results as `mix_pass` establishes stem sample 0 —
  `keep_from_frames = frame_to_samples(range.start)` (export.rs:1218-1226), the spelling
  `measure_mix_window_levels`' own `requested_frames` repeats — rather
  than with a private re-spelling of the same formula. The indices are only meaningful against the
  grid `measure_mix_window_levels` actually lays down, so one conversion has to move both or
  neither — R85's argument applied across a crate boundary — and converting before subtracting
  keeps the pair exact at a non-integer sample-per-frame rate such as 30000/1001, where truncating
  a difference loses a sample, and `au5_clip_window_levels_offsets_against_a_non_zero_range_start`
  pins that one sample directly: at 30000/1001 a report starting at frame 2 puts frame 5 at stem
  sample 4 805 converted-then-subtracted against 4 804 the other way, which against a 100 ms window
  is a whole window index (2 against 1). (ii) The in-crate double answers `base − index` hundredths
  rather
  than a flat level, so the wholly-inside rule is **falsifiable**: the fade test asserts each clip's
  head and tail at their computed window indices (clip 2 → 0 and 99, clip 3 → 100 and 109, clip 4 →
  110 and 115 at the 20 ms default), which is what proves that the cut at sample 96 000 falls
  exactly between window 99 and window 100 and that window 116 is excluded for straddling clip 4's
  end. A flat level satisfies any index arithmetic at all, including a wrong one.

**Implementation errata, Part A app (2026-09-10).** R90–R99 is the app's reserved range; it is
appended here, ahead of the closing line below, so three implementers editing §0 at once cannot
collide; the numbers below R90 belong to the other Part A crates, whose blocks follow the closing
line. A reviewer reading §0 straight through should read this block last.

- **R90. A13's `has_gain_computer` pin lives in the app, and reads media's source as text.**
  A13 asks for "an `include_str!` count over both files" without saying which crate owns the
  assertion. It landed as `au5_has_gain_computer_is_declared_in_neither_crate_that_used_to_own_it`
  in `mixer_pane_ui.rs`'s test module, because the app is the only Part A crate that can name both
  paths — `crates/kinewright-media/src/audio.rs` resolves from the app's `src/` as
  `../../kinewright-media/src/audio.rs`, and the app already depends on `kinewright-media`, so the
  pin adds no Cargo edge, only a compile-time textual one. Two consequences worth knowing before
  reading it: `mixer_pane_ui.rs` is read **only up to its `#[cfg(test)]` line** — the house shape
  `timeline_ui.rs:3813` already uses — because the test module quotes the very name the pin
  searches for, while media's `audio.rs` is read whole, since nothing anywhere in that file may
  declare the function; and the pin **fails until the media half of R14 lands**, which is correct
  and is the point of writing it as an assertion rather than as a grep in a review checklist.
  It also asserts `has_gain_computer("audio_denoise")` through the app's own import, so a core
  predicate that forgot the denoiser could not pass while the two copies stayed deleted, and it
  counts the **two call sites** — `has_gain_computer(&effect.name)` at the header readout and at
  `reduction_bar`'s gate — at exactly 2. That second count carries rule 84's other half ("both
  callers read the core one") and is what makes the truncation non-vacuous by construction: the
  call sites sit in the middle of the production half, so a `#[cfg(test)]` item landing earlier in
  the file some day would shorten the searched region past them and fail loudly rather than pass on
  a prefix that no longer covers the ground the deleted copy stood on.
- **R91. The three forwarding tests share one double and one proxy constructor.** Rule 86 asks for
  three tests "using a double that answers **typed** errors". Rather than three doubles, AU3's
  existing `ShortRangeAnalysis` (color_qc_ui.rs) gains the three methods and a `short_range_proxy`
  helper builds the wrapper the three tests share; the AU3 test that predates them is left
  building its proxy inline and is untouched. `audio_repair` deliberately answers
  `MixSpectrumRangeTooShort` where `mix_noise_profile` answers `MixLoudnessRangeTooShort`. Not
  because a cross-wired arm is reachable **today** — `MixNoiseProfileRequest` and
  `AudioRepairRequest` are structurally identical but nominally distinct types, as are
  `NoiseProfileReport` and `AudioRepairReport`, so forwarding one from inside the other fails to
  compile on both the argument and the return type — but because the day somebody de-duplicates
  that identical pair into one type or alias, the mistake type-checks. Two different variants make
  it falsifiable in advance, at the cost of one word. The middle test, which shares
  `MixLoudnessRangeTooShort` with its neighbour, is separated **by value** instead: the double
  echoes the request's own fields, and a legal `hop_milliseconds` lies in `1..=window ≤ 1000`
  while every point code is `0` or `≥ 1_000_000`, so the two producers cannot meet.

No OPEN note remains.

## 1. Scope

### 1.1 The editor job

After AU4 the editor can balance, process, ride and verify a mix, but every one of those tools
assumes the recording is already usable. They cannot take hiss out of a location interview, get a
50 Hz buzz off a lav, kill the mouth clicks in a podcast, or stop a dialogue cut from dropping into
digital silence — the one artefact that makes a cut audible even when the picture is perfect. AU5
adds three repair nodes to the chain vocabulary, a way to learn what the noise *is* from the
recording itself, a room-tone asset and a fill for cut gaps, one inspector that measures whether a
repair actually helped, and two planners that refuse when it did not.

### 1.2 The two parts

**Part A — Repair nodes and measurement.** The three descriptors and their rows (§2.1); the single
static-parameter rule and the amended AU2 reason string (§2.2); validation, the 20 ms budget and the
one new QA finding (§2.3); `core/src/audio_repair.rs` (§2.4); the twiddle table and the in-house
inverse FFT (§3.1); the denoiser, the profile unit, the hum cascade and the de-click detector
(§3.2–§3.5); `node_lookahead_milliseconds` reading the descriptor (§3.6); the three new `Analysis`
measurements (§3.7–§3.9); the numeric contract and both-paths parity (§3.10); the fixtures and their
budgets (§3.11); `get_audio_repair` (§4.1); the profile hatch and the compact render arm (§4.2); the
Part A ledger (§4.3); `has_gain_computer` in core (§4.4); the three `BaselineProofAnalysis` arms and
the retirement of `plan_clip_fades`' two-render hack (§4.5). **No new `Operation` variant and no new
editing surface**; the app work is three forwarding arms and one deleted duplicate. Commit
`feat: complete AU5a repair nodes and measurement`.

**Part B — Room tone and repair planners.** The room-tone store (§5.1); `Document::track_gaps` and
the `qa_document` refactor (§5.2); the fill arithmetic and the seam fixture (§5.3, §5.4); the two
planners and the `normalization_context` amendment (§5.5–§5.7); `capture_room_tone` and the Part B
ledger (§5.8, §5.9); the three Mixer cards, the wells, the `Learn` gesture, the timeline `Room tone`
button, the height pins and DESIGN.md (§6.1–§6.5). Commit
`feat: complete AU5b room tone and repair planners`.

**How the roadmap row is discharged.** The AU5 row's **exit gate**
(ROADMAP-AND-WORKFLOWS.md:649) has two clauses. (1) *"Repair is measured on synthetic corruptions
with pinned SNR gains"* — closed entirely by **Part A** (A7–A11, A15). (2) *"fills are seamless at
1e-4 across the join"* — closed entirely by **Part B** (B4–B6). The row's **deliverables** —
*"Broadband noise reduction with a learned profile, hum removal, de-click, room-tone capture and
fill for cut gaps, dialogue isolation where the model can measure improvement"* — split across the
two: the three nodes, the learned profile and the whole of the measurement are Part A; capture, fill
and the two planners are Part B. "Dialogue isolation where the model can measure improvement" is the
Part A inspector plus the Part B planner, and the *measurement* — which is the clause that gates
it — is Part A, which is why Part A is a complete exit gate on clause 1 and lands first. Part A also
pays off AU4's deferred short-window RMS accessor, named in situ at server.rs:8776-8783 (§3.8).

### 1.3 Out of scope (named deferrals)

Source separation and any model-based dialogue isolation — the row says "dialogue isolation **where
the model can measure improvement**", and §5.6 reads that as a measured combination of the three
nodes; AU5 ships no separation model and this sentence exists so nobody reads the row as one.
De-reverb; de-clipping and spectral repair of clipped audio; de-essing and plosive removal. Adaptive
or tracking hum, because an estimator that drifts onto a bass note is worse than a notch in the
wrong place the editor can see. Per-clip repair nodes and any relaxation of `AudioEffectOnClip`
(operation.rs:556, raised :3208 and :4476). Automation of the noise profile — the 31 bands stay
static — and real-time learning while playing. A cached, asset-domain `AnalysisKind::NoiseProfile`,
revisited only if §3.7's re-measure proves slow on real material. A crossfade primitive and paired
ramps; room-tone tiles that alternate or reverse to hide periodicity; a gap hit target, badge or
context menu; a dedicated room-tone track and its routing. Dropouts longer than
`max_click_milliseconds`. Multi-band downward expansion. Renegotiating
`CHAIN_LOOKAHEAD_MILLISECONDS`. A `MixerUnit` for reduction — the existing `reduction_bar` covers
it. Any `forward_fft` performance work beyond R5's twiddle table. And every AU6 workflow definition,
prompt, eval case and blind review (ROADMAP-AND-WORKFLOWS.md:650).

### 1.4 Documentation

**Part A:** this contract; `CHANGELOG.md` — one `### Added` line for the three repair nodes and the
three measurements, one `### Changed` line for "an audio node's declared latency is now read from
its descriptor in the mix path as well as in validation, so a node whose `lookahead_milliseconds` is
absent from the document no longer runs with zero delay"; `MEDIA-POLICY.md` "Playback audio mixdown"
(:106-141) gains one paragraph on the STFT block grid, the preroll it implies and the three declared
delays; `AU2-EQ-AND-DYNAMICS.md` §0 gains **two** errata — R6's amended reason string and **R40's
narrowing of the bypass rule for `audio_denoise`** — and §2.2's reason string is amended in place
with an AU5 pointer sentence in AU1 §0 style; `AU3-LOUDNESS-AND-DELIVERY.md` §5.8 gains a pointer
sentence naming AU5's fixture lane as its descendant; `M36-AGENT-RUNTIME-EFFICIENCY.md:93-120` gains
a Part A row. **`DESIGN.md` is not touched in Part A** — no new surface.

**Part B:** this contract; `CHANGELOG.md` — one `### Added` line for room-tone capture and fill, one
for the two planners, and a `### Changed` line for "`plan_audio_normalization` now extends a bus that
carries only AU5 repair nodes instead of refusing it"; `ROADMAP-AND-WORKFLOWS.md`'s status paragraph
names the two parts and the clause split; `DESIGN.md` `### Mixer` and `### Timeline` (§6.5);
`README.md` line 36; `MEDIA-POLICY.md` gains one sentence on the room-tone store beside the LUT
store's; `AU3-LOUDNESS-AND-DELIVERY.md` §6.3 gains a pointer to §5.7; `M36` gains three Part B rows.

---

# Part A — Repair nodes and measurement

---

## 2. Part A core model

### 2.1 The three descriptors (D1, D2, R3, R4, R17)

`EFFECT_DESCRIPTORS` (effect.rs:1395) grows from 25 to **28** entries; the three new audio
descriptors are appended after `audio_true_peak_limiter` (effect.rs:2052-2089). `is_audio_effect`
(effect.rs:3075-3088) extends its `matches!` to **eleven** names. **Forty-five new
`EffectParameterDescriptor` rows** (36 + 5 + 4, counting the shared `bypass` once per descriptor)
and **twelve new `EffectUniform` variants**, because the three `bypass` rows reuse
`AUDIO_BYPASS_DESCRIPTOR`'s `EffectUniform::AudioBypass` and all 31 profile rows share one
`EffectUniform::DenoiseProfileBand`. All twelve join the compositor's exhaustive ignore arm
(compositor.rs:2658-2672).

1. **Normative.** All three repairs are **chain nodes** on a bus or master chain.
   `AudioEffectOnClip` stays. Nothing in AU5 needs per-clip state: hiss, hum and clicks are
   properties of a *source*, and a source reaches one bus through one track — `validate_audio_mix`
   raises `TrackInMultipleAudioBuses` (operation.rs:4611-4619). *Rejected: a per-clip repair node* —
   it would need a §0 ruling against `AudioEffectOnClip`, a per-source DSP seam that does not exist
   (`ClipAudioShaping` is a scalar gain per project sample with nowhere to hold filter state,
   audio.rs:444-579), chunk-boundary and seek rules inside `AudioMixSource`, and a survival policy
   for a non-curve clip field that `split_clip` and `replace_clip` would otherwise clone onto
   entirely different media. *Rejected: a `Clip.noise_profile` field* — 146 literal edits, a
   `validate_clip_audio` arm, a survival rule and a render suffix, for a setting that describes the
   microphone, not the take.
2. **Normative.** Nothing requires an `EffectUniform` to be unique per descriptor —
   `EffectUniform`'s own doc says so (effect.rs:65-71), `AudioBypass` already serves eight `bypass`
   rows and `ColorNode` every colour-node parameter. A 31-row block therefore costs **one** variant,
   not 31.
3. Every unit is in the parameter name, because the app reads the suffix (`mixer_unit`,
   mixer_pane_ui.rs:1301-1317) and because ROADMAP:630-633 makes integer document controls with
   stable units the rule. Every node carries `AUDIO_BYPASS_DESCRIPTOR`.

**`audio_denoise` — 36 parameters** (5 rows plus 31 profile rows).

| name | min | max | neutral | unit | uniform | semantics |
| --- | --- | --- | --- | --- | --- | --- |
| `bypass` | 0 | 1 | 0 | flag | `AudioBypass` (shared) | the gain law is skipped and the node's delay line still applies, so bypass never changes `L` (AU2 §2.1). Hold-only |
| `reduction_tenth_db` | 0 | 400 | **0** | tenth dB | `DenoiseReduction` | maximum attenuation `R` any bin may receive, as a positive figure. **0 is an identity**: the gain floor is `10^(0/20) = 1`, so every bin gain is exactly 1 |
| `floor_offset_tenth_db` | -200 | 200 | 0 | tenth dB | `DenoiseFloorOffset` | over- or under-subtraction of the learned floor, `a = 10^(offset/200)` |
| `smoothing_milliseconds` | 0 | 200 | 50 | ms | `DenoiseSmoothing` | one-pole time constant of the per-bin gain across blocks; 0 is no smoothing |
| `lookahead_milliseconds` | **12** | **12** | 12 | ms | `DenoiseLookahead` | the node's whole algorithmic delay. **Static** (§2.2), never keyed, and a one-value domain so the editor cannot spend the budget differently |
| `profile_band01_tenth_db` … `profile_band31_tenth_db` | -1200 | 0 | **-1200** | tenth dBFS | `DenoiseProfileBand` (one shared variant) | the learned noise floor at each ISO third-octave centre, low to high, in the unit `band_level_hundredths` reports (§3.3). **Static**. Write all 31 or none |

**`audio_hum_removal` — 5 parameters.** All-neutral is an exact identity: `depth_tenth_db = 0`
makes every `peaking` section's numerator and denominator bitwise equal, so the cascade is a
pass-through sample for sample (§3.4 rule 49).

| name | min | max | neutral | unit | uniform | semantics |
| --- | --- | --- | --- | --- | --- | --- |
| `bypass` | 0 | 1 | 0 | flag | `AudioBypass` (shared) | as above. Hold-only |
| `fundamental_hertz` | 50 | 60 | 50 | Hz | `HumFundamental` | mains frequency. Fixed, never tracked (§1.3) |
| `harmonic_count` | 1 | 10 | 1 | count | `HumHarmonicCount` | how many sections, at `h × fundamental_hertz` for `h in 1..=count`. **Hold-only**: a fractional number of notches is meaningless |
| `depth_tenth_db` | -600 | **0** | **0** | tenth dB | `HumDepth` | each section's peaking gain, as a negative figure. **0 is an identity** |
| `notch_q_hundredths` | 100 | 1800 | 1200 | hundredths | `HumNotchQ` | each section's Q, 1.00 … 18.00. Named for the `_q_hundredths` suffix (R17); Q 12 at 50 Hz is a 4.17 Hz bandwidth |

**`audio_declick` — 4 parameters.** All-neutral is an exact identity: `max_click_milliseconds = 0`
means no span is short enough to qualify, so the detector runs, flags nothing, repairs nothing, and
the output is the delayed input bit for bit.

| name | min | max | neutral | unit | uniform | semantics |
| --- | --- | --- | --- | --- | --- | --- |
| `bypass` | 0 | 1 | 0 | flag | `AudioBypass` (shared) | as above. Hold-only |
| `max_click_milliseconds` | 0 | **1** | **0** | ms | `DeclickMaxClick` | the longest flagged span the node will repair. A longer span is a dropout, not a click, and is left alone. **Not static** (R6): the detector is sized from this row's **maximum** |
| `detector_threshold_tenth_db` | 60 | 400 | 240 | tenth dB | `DeclickThreshold` | how far above the trailing `d2` reference a sample must sit to be flagged; `10^(value/200)` as an amplitude ratio |
| `lookahead_milliseconds` | **3** | **3** | 3 | ms | `DeclickLookahead` | the node's whole algorithmic delay. **Static**, never keyed |

4. **Normative, on neutrality.** "Every audio descriptor's neutral is an identity" is already false
   in the workspace — `audio_limiter`'s `ceiling_tenth_db` neutral is −10 and `audio_ducking`'s
   neutrals are a working duck (AU2 §2.1) — but it is **true of all three AU5 nodes**, and each
   node's identity is *structural*, not approximate: an all-neutral insert is a pass-through with
   its delay line already correct, exactly as `audio_compressor`'s ratio-1:1 neutral is. §3.10 rule
   72 pins it bit for bit through `process_buffer_static`.
5. **Normative (R4).** The 31 profile neutrals are **−1200**, and the runtime rule is *all 31 at the
   neutral ⇒ unity gain*. This is a wire-format decision, not a runtime convenience: at a neutral of
   0 the gain law's numerator `m − a·floor` is negative for every real signal, so `g` pins at the
   floor and an unlearned node attenuates the whole signal by the full `reduction_tenth_db` while
   `export_ready` — which counts errors only — ships it. A −120 dB floor gates nothing, so the
   identity survives any combination of `reduction_tenth_db`; omit-defaults still omits an unlearned
   node's 31 rows, so every existing render golden is byte-unchanged; and §2.3's
   `noise_profile_missing` warning stays as advice on top.
6. **`insert_audio_effect` skips the profile rows.** `insert_audio_effect`
   (mixer_pane_ui.rs:1653-1685) writes **every** descriptor parameter explicitly at its neutral. For
   `audio_denoise` it skips `is_noise_profile_parameter(name)` rows: the 31 bands are a
   write-all-or-none block whose absence is exactly the neutral, so writing them would put 31
   `-1200` entries into every document that inserts the node and would keep §4.2's compact-render
   omission arm from ever firing in the app's own documents.
7. **One new core predicate.** `is_noise_profile_parameter(name: &str) -> bool` in `effect.rs`,
   `pub`, re-exported from `lib.rs`: `name.starts_with("profile_band") && name.ends_with("_tenth_db")`.
   It is the single definition read by `is_static_audio_parameter`, `effect_documentation`,
   `render_effects`, `insert_audio_effect` and `card_body`, so "which rows are the profile" cannot
   drift between five files.
8. *Rejected: a `profile_id` static parameter referencing a derived artefact* — the node lives on a
   bus, which has no asset; an id would need a document-side registry that does not exist and a
   lookup the runtime cannot perform, because `AudioEffectRuntime::new` (audio.rs:815) sees an
   `&Effect`, a channel count and a rate and nothing else. *Rejected: an `AudioBus.noise_profile`
   field* — `AudioBus` is `Operation`-reachable through `UpsertAudioBus`, so a new struct there costs
   ≈ 1.2 kB × 54 tools ≈ 65 kB of `input_schema_bytes`, and the runtime still cannot see it.
   *Rejected: a store artefact on the `LutAsset` shape* — a 31-integer table is 300 bytes and the
   digest-plus-store shape exists for megabytes. *Rejected: 15 half-octave bands* — kept in reserve
   (R27); it halves §3.3's interpolation error and §4.2's render bytes, but also the resolution where
   hum and rumble live, which is where the profile earns its keep.

### 2.2 Static and hold-only parameters, and the amended reason string (R6, R17)

9. **Normative, the single static-parameter rule.** *A static parameter takes no keyframe. The
   runtime reads static parameters at construction and **re-derives** them on every
   `parameter_epoch` bump, except those in `node_structure`, which force a rebuild instead.* Today's
   behaviour is the second half only: `node_structure` is `(EffectId, &name, static lookahead)`
   (audio.rs:1345-1358) and its doc records that `rms_window_milliseconds` is deliberately *not* in
   the key, so a live edit of it is silently ignored until a rebuild while export builds fresh. AU5
   does not change that for AU2's parameters; it states the rule once and puts the profile bands on
   the **re-derive** side, which is what makes a `Learn` audible without a re-cue (rule 73).
10. **Two edits, not three arms (R36).** `is_static_audio_parameter` (effect.rs:3102-3110) matches
    on the **parameter name first**, then the effect name, so AU5 makes exactly two edits, not three
    arms: the `AUDIO_LOOKAHEAD_PARAMETER` arm's inner `matches!` gains
    `| "audio_denoise" | "audio_declick"`, and a new guarded arm
    `p if is_noise_profile_parameter(p) => effect == "audio_denoise"` is inserted **before** the
    `_ => false` wildcard. There is no tuple match to add an arm to. The predicate is then true for
    exactly five pairs' worth of names — the three pre-AU5 pairs plus the two new lookahead rows —
    and for every one of `audio_denoise`'s 31 profile rows. The two lookahead arms are
    load-bearing twice over: `chain_lookahead_milliseconds` (model.rs:735-755) sums
    `lookahead_milliseconds` **only** over effects the predicate accepts, and §3.6's
    `node_lookahead_milliseconds` reads the same predicate.
11. **`max_click_milliseconds` is deliberately absent from the predicate** (R6). It is a threshold,
    not an allocation: the de-click detector's ring, guard and repair buffer are sized from the
    descriptor's **maximum** (1 ms) at construction, so a per-frame read allocates nothing. Making it
    static would have inherited AU2's E16 divergence — silently ignored live, honoured in export —
    for a control an editor will reach for while listening.
12. `is_hold_only_parameter`'s audio branch — the `matches!` at operation.rs:4036-4038 (R46 corrects
    the draft's :4032-4059) — gains **`harmonic_count`**, so
    it reads `bypass | detector | true_peak | harmonic_count` for any `is_audio_effect` name; a
    non-`Hold` interpolation on it raises the existing `NonHoldKeyframeParameter`.
13. **The amended reason string (R6).** `validate_audio_chain_automation` (operation.rs:4817-4901)
    keeps its two distinct rejections, but the second's text changes from
    `"is read once when the chain is built and cannot be keyframed"` to
    **`"is read when the chain is built or retuned and cannot be keyframed"`**, because rule 9 makes
    the old sentence false for the profile. The string is pinned in exactly three places, all
    updated in Part A: `crates/kinewright-core/src/operation.rs:4835`,
    `crates/kinewright-core/tests/au2_core.rs:634` and `docs/AU2-EQ-AND-DYNAMICS.md:99`. AU2 §0
    records the change as an erratum with a pointer to this rule. The **first** reason,
    `"sets processing latency and cannot be keyframed"`, is unchanged and is the one the two new
    `lookahead_milliseconds` rows raise.

### 2.3 Validation, the 20 ms budget, and the one new QA finding

14. **AU5 adds no `OpError` variant.** Every failure it can cause is an existing one: a band outside
    `-1200..=0` is a descriptor-domain failure, a keyframe on a static parameter is
    `InvalidEffectAutomation` with one of rule 13's two reasons, a non-`Hold` `harmonic_count` key is
    `NonHoldKeyframeParameter`, an over-budget chain is `AudioBusLookaheadExceeded` or
    `AudioMasterLookaheadExceeded`, a fill on an unregistered asset is `MissingAsset`, and a fill
    that overruns its neighbour is `ClipOverlap`.
15. **AU5 adds no `Operation` variant.** Repair rides `UpsertAudioBus` / `SetAudioMaster`; the fill
    rides `AddAsset` + `AddClip`, both of which are ordinary generated tools —
    `UNGENERATED_OPERATION_VARIANTS` is exactly `["RelinkAsset", "AddLutAsset", "ConvertLegacyLook"]`
    (schema.rs:135) and `Operation::AddAsset { asset }` maps to `"add_asset"` (schema.rs:270).
    Generated tools stay **54** and the served quad cannot move (§4.3).
16. **The latency budget stays 20 ms and the arithmetic is stated, not stretched.** The three nodes
    declare **12 + 0 + 3 = 15 ms**. `audio_hum_removal` has no `lookahead_milliseconds` row at all,
    so both core's sum and media's per-node read (§3.6) contribute exactly 0 for it — which is the
    B1 test's sharpest case.

    | chain | nodes | declared |
    | --- | --- | --- |
    | repaired dialogue bus | denoise 12 + hum 0 + declick 3 | **15** ≤ 20 ✓ |
    | the same bus after `plan_audio_normalization` (§5.7) | + compressor 0 + true-peak limiter 5 | **20** = 20 ✓ |
    | repair + a 5 ms lookahead compressor, no bus limiter | 12 + 0 + 3 + 5 | **20** ✓ |
    | AU2's worst legal chain (compressor 10 + limiter 10) + any repair | 20 + 15 | **35** ✗ |
    | master delivery chain | true-peak limiter 5 | 5 ≤ 20 ✓ (its own budget) |

    The fourth row is a real refusal, already spelled and already surfaced:
    `AudioBusLookaheadExceeded { bus, milliseconds }` (operation.rs:4768-4774), and the app's
    `insertion_block` greys `+ Effect` with `LOOKAHEAD_BUDGET_SPENT` (mixer_pane_ui.rs:1623-1625)
    from the descriptor, so a `12..=12` row greys the denoiser correctly with no app change. **The
    constant is not amended.** For the record, the blast radius if it were:
    `CHAIN_LOOKAHEAD_MILLISECONDS` (model.rs:708), the two `OpError` messages, AU2 §3.6's prose and
    its `include_str!` pins, AU4 §3.5's per-owner automation offsets (R25) and their pinned numbers,
    the app's `insertion_block`, and every test asserting 20.
17. **Validation is the existing path**, unchanged: descriptor domains, rule 13's two static
    rejections, `is_hold_only_parameter`, `chain_lookahead_milliseconds` against the budget,
    `validate_asset` for the captured WAV, and `validate_source_range` plus `ClipOverlap` for the
    fill. AU5 adds no new `validate_*` function in Part A.
18. **One new QA finding.** `qa_document` raises **`"noise_profile_missing"`** at
    `QaSeverity::Warning` for every `audio_denoise` node — bus or master — whose `reduction_tenth_db`
    resolves above 0 and whose 31 bands are all at the neutral: `"denoise node {effect} on {owner}
    has a reduction of {r} tenth dB but no learned profile; learn one or the node does nothing"`. It
    does **not** block export — `export_ready` counts `Error` only (qa.rs:53-56) — consistent with
    `"track_gap"` and correct, because R4 has already made the configuration harmless rather than
    destructive. It is advice about a wasted node, not a gate.
19. **Undo.** One gesture, one entry. The `Learn` gesture is one `UpsertAudioBus` / `SetAudioMaster`
    under the existing `audio_bus:{id}` / `audio_master` coalesce keys (mixer_ui.rs:308, :313); the
    timeline `Room tone` button is one `DoBatch` of `[AddAsset?, AddClip…]`; `capture_room_tone` is
    one `AddAsset`. **No new coalesce key.**

### 2.4 `core/src/audio_repair.rs` (D13, R18)

New module beside `audio_qc.rs`, re-exported from `lib.rs`, under the same module contract its doc
comment (audio_qc.rs:1-7) states: media measures and core judges, every threshold is a `const` and
not a parameter, every number is an integer with its unit in the field name, and a report is
evidence — it carries `evidence_only`, deliberately never `export_ready`.

Both types derive `Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema` with
`#[serde(rename_all = "snake_case")]`, the request adding `deny_unknown_fields`; every `Option` and
`Vec` field carries N0's standing optional attributes
`#[serde(default, skip_serializing_if = …)] #[schemars(default)]`.

```rust
pub struct AudioRepairRequest { range: Option<Range<TimeCode>>, point: MixSpectrumPoint }

pub struct AudioRepairReport {
    range: Range<TimeCode>, point: MixSpectrumPoint, sample_rate: u32, sample_frames: u64,
    /// The RMS window the percentiles are taken over (§3.8), and how many
    /// windows carried energy above `SILENCE_POWER`.
    window_milliseconds: u32, windows: u32,
    /// The 10th percentile — **a percentile, not a detected floor** — the 90th,
    /// and their difference.
    noise_floor_dbfs_hundredths: Option<i32>, signal_dbfs_hundredths: Option<i32>,
    snr_db_hundredths: Option<i32>,
    /// Excess over the sixth-octave shoulders, summed over four harmonics, and
    /// the four per-harmonic values low to high (four entries or none).
    hum_50_excess_db_hundredths: Option<i32>, hum_60_excess_db_hundredths: Option<i32>,
    hum_50_harmonic_excess_db_hundredths: Vec<i32>, hum_60_harmonic_excess_db_hundredths: Vec<i32>,
    click_count: u32, click_density_per_minute: u32,
    findings: Vec<AudioQcException>, evidence_only: bool, provenance: AudioRepairProvenance,
}
```

20. **Every leaf is an integer.** No `f32`/`f64` appears on any AU5 serialized type, so all of them
    derive `Eq` — which is itself the proof, on the rule facts-core §3 records for the whole
    derived-data family.
21. **The SNR definition, normatively (R18).** Over §3.8's `REPAIR_WINDOW_MILLISECONDS = 10`
    windows at the chosen point, `noise_floor_dbfs_hundredths` is the **10th-percentile** window
    level and `signal_dbfs_hundredths` the **90th**, both by `nearest_rank_index`
    (loudness.rs:219-222), and `snr_db_hundredths = signal − noise_floor`. Percentiles rather than a
    silence detector: there is no threshold to tune and it degrades gracefully on material with no
    silence in it. **Its bias is stated wherever it is published.** On material with real silence the
    10th/90th split is right; on continuous speech the 10th percentile is *quiet speech*, so the
    floor reads high and the SNR low — the safe direction, but a planner that did not know it would
    refuse a repair that worked. `get_audio_repair`'s first sentence (§4.1) and the rendered report
    both say "percentile", and `AudioRepairReport`'s doc comment carries this paragraph.
22. All three percentile fields are `None` below `REPAIR_MINIMUM_WINDOWS = 10` energetic windows and
    the report says so through a `low_window_count` finding rather than inventing a number.
23. `AudioRepairProvenance` follows `AudioQcProvenance` (audio_qc.rs:69-97): fixed strings with a
    manual `Default`, and `AUDIO_REPAIR_ENGINE = "kinewright_audio_repair_v1"`.
24. **Core judges.** `audio_repair_exceptions(&AudioRepairMeasurements) -> Vec<AudioQcException>`
    mirrors `audio_qc_exceptions` (audio_qc.rs:172+) — same sort order `(severity desc, code asc,
    field asc)`, same exception shape — over four `const` thresholds:
    `REPAIR_LOW_SNR_HUNDREDTHS = 1_200` (an SNR under 12 dB is a `low_signal_to_noise` warning),
    `REPAIR_HUM_EXCESS_HUNDREDTHS = 600` (`mains_hum_present`),
    `REPAIR_CLICK_DENSITY_PER_MINUTE = 30` (`click_density_high`), and `REPAIR_MINIMUM_WINDOWS`
    (`low_window_count`, Info). `AudioRepairMeasurements` is the **non-serde** struct media hands
    core, exactly as `AudioQcMeasurements` (audio_qc.rs:147-163) is.
25. **The point type is reused, not re-spelled.** All three new requests carry
    `point: MixSpectrumPoint` (media.rs:1245-1252 — `Master | Track(TrackId) | Bus(AudioBusId)`),
    the type `mix_spectrum` already uses. AU5 adds no fourth spelling of "where in the graph".

---

## 3. Part A media

### 3.1 The inverse FFT and the twiddle table (D3, R5, R19)

26. **The inverse comes free.** `ifft(X) = conj(forward_fft(conj(X))) / N`, six lines around the
    audited routine, **exact** because `forward_fft` (spectrum.rs:62-105) is an unnormalised forward
    DFT over `f64` `Complex` pairs. No new transform, no new crate — every workspace dependency is
    pinned with `=` (Cargo.toml:19-42) and spectrum.rs:1-6 states the no-new-dependency rule.

    ```rust
    /// AU5 §3.1: the normalised inverse of `forward_fft`, by conjugation.
    ///
    /// `ifft(X)[n] = conj(fft(conj(X))[n]) / N`. Exact up to rounding because
    /// `forward_fft` is unnormalised: the only arithmetic added is a sign flip
    /// on the imaginary parts and one division per bin.
    pub(crate) fn inverse_fft(data: &mut [Complex]);
    ```
27. `Complex` (spectrum.rs:43-46 — R46 corrects the draft's :41-58) and `forward_fft` are already
    `pub(crate)` and reachable from `audio.rs`. `hann_window` (spectrum.rs:253-267) and
    `SILENCE_POWER` (spectrum.rs:36) become `pub(crate)` — two visibility changes (R21, R34) — and
    `hann_window` stays the periodic `f64` window it is, so the denoiser's analysis window and
    `third_octave_spectrum`'s are literally the same numbers.
28. **The twiddle table (R5), and why it is a Part A obligation.** `needs_seek_preroll`
    (audio.rs:2925-2935) is true whenever any bus exists **or** the master chain is non-empty and
    `project_from > 0`, and `open` then sets `decode_from = TimeCode::ZERO` (:2961-2966) — existing
    AU2 behaviour, and what makes §3.10's parity structural. But with a denoiser on the chain every
    prerolled frame now runs an STFT: measured on this machine (release build, standalone replica),
    **17.1 µs per 512-point transform → 25.6 ms of CPU per second of 48 kHz stereo → ≈ 15 s of pure
    FFT for a seek at minute 10**, because `forward_fft` calls `sin_cos` **per butterfly**
    (spectrum.rs:82-88).

    ```rust
    /// AU5 §3.1: per-stage twiddles, computed once per stage size.
    ///
    /// The table for stage size `m` holds `e^(-2πik/m)` for `k in 0..m/2`,
    /// computed by the **identical expression** the butterfly loop used
    /// before — `(-2.0 * PI / m as f64 * k as f64).sin_cos()` — so every
    /// transform in the workspace is bit-identical to its pre-AU5 result and
    /// AU2's spectrum goldens do not move.
    fn stage_twiddles(stage: usize) -> &'static [Complex];

    /// Largest stage this table serves: 2^17, comfortably over the 16 384-point
    /// Welch window `third_octave_spectrum` uses.
    const FFT_MAX_STAGE_LOG2: usize = 17;
    static STAGE_TWIDDLES: [OnceLock<Vec<Complex>>; FFT_MAX_STAGE_LOG2 + 1] =
        [const { OnceLock::new() }; FFT_MAX_STAGE_LOG2 + 1];
    ```
29. **Normative on bit-identity.** The table is keyed by **stage size**, not by transform length,
    precisely so the float argument handed to `sin_cos` is unchanged. A table keyed by transform
    length would have to compute `e^(-2πi(kN/m)/N)`, whose argument differs from `e^(-2πik/m)` in
    the last ulp for some `k`, and that would move `measure_mix_spectrum`'s goldens. A Part A test
    runs both routines over pseudo-random buffers at lengths 512, 4 096 and 16 384 and asserts
    `to_bits()` equality on every bin (A5).
30. **The preroll lane is printed, not gated (R5).** A media test measures the wall-clock cost of one
    second of 48 kHz stereo through a denoise-bearing chain, with and without the table, and prints
    `AU5_PREROLL seconds=… stft_milliseconds=… ratio=… speedup=…`. It carries **no timing assert** —
    a timing gate would be flaky or OS-conditioned, and N0 forbids OS-conditional tolerances — so the
    seek cliff is evidence in the log rather than a support ticket. **Skipping block work during
    preroll is not adopted**: the OLA accumulator and the per-bin smoother are stateful across
    blocks, so a skipped preroll would desynchronise them and break §3.10's parity.

### 3.2 `audio_denoise`: the STFT gate (D3, R19, R38, R39, R40, R49)

31. **Normative.** A 512-frame periodic-Hann window with a 128-frame hop (75 % overlap) at 48 kHz,
    analysis and synthesis both windowed, reconstructed by overlap-add and divided by the Hann²
    overlap constant **1.5**, which is exact for a periodic Hann at hop = N/4 (the cos θ and cos 2θ
    terms cancel over the four shifts — R19).
32. **Rate independence, and the exact latency (R19).** At construction the node computes
    `latency = stage_latency_frames(node_lookahead_milliseconds(effect), rate)` — 12 ms, read from
    the descriptor by §3.6 — then `window =` the largest power of two `≤ latency`, `hop = window / 4`,
    and `pad = latency − (window − 1)`. The OLA delay is `window − 1`, **not** `window`: an output
    sample at `p` is complete only once the window *starting* at `p` has been added.

    | rate | `stage_latency_frames(12)` | window | hop | OLA delay | pad | window in ms |
    | --- | --- | --- | --- | --- | --- | --- |
    | 44 100 | 529 | 512 | 128 | 511 | **18** | 11.61 |
    | 48 000 | 576 | 512 | 128 | 511 | **65** | 10.67 |
    | 96 000 | 1 152 | 1 024 | 256 | 1 023 | **129** | 10.67 |

    Export and every mix measurement run at 48 kHz (export.rs:37-38), so every pinned number in §3.11
    is the 48 kHz row. *Rejected: a 1 024-frame window at 48 kHz* — 21.3 ms, over the whole 20 ms
    budget on its own. *Rejected: declaring 11 ms* — `floor(11 × 44 100 / 1000) = 485`, so the
    largest power of two `≤ 485` is **256**, halving the resolution at 44.1 kHz; 12 ms is the
    smallest declaration that holds a 512-frame window at every rate ≥ 44.1 kHz (R19 corrects the
    brief's reason: a node cannot under-declare a delay it derives from its own declaration).
33. **The state machine.** `AudioEffectState` (audio.rs:783-796) gains one variant:

    ```rust
    Denoise(Box<DenoiseState>),

    struct DenoiseState {
        ring: Vec<Vec<f64>>,          // per channel, the newest `window` input frames
        cursor: usize, since_block: usize,
        accumulator: Vec<Vec<f64>>,   // per channel, `window` f64, rotated by `hop`
        smoother: Vec<Vec<f64>>,      // per channel, `window / 2 + 1` smoothed bin gains
        scratch: Vec<Complex>,        // one `window`-point buffer, reused per channel
        window: Vec<f64>,             // the periodic Hann, analysis and synthesis
        bin_floor_power: Vec<f64>,    // re-derived at construction and on epoch (§3.3)
        bypass_delay: DelayLine,      // the bit-exact identity path (rule 41)
        output_pad: DelayLine,        // the residual `latency − (window − 1)` after the OLA
        direct: bool, direct_switch_remaining: usize,
        minimum_gain: f32,            // published by `take_gain_reduction` (§4.4)
        cached: Option<(TimeCode, u64)>,
    }
    ```
34. **Scratch, honestly (R19).** Per channel: the input ring (512 f64 = 4 kB), the OLA accumulator
    (512 f64 = 4 kB) and the per-bin smoother (257 f64 ≈ 2 kB) ≈ **10 kB**, so **20 kB** for stereo;
    plus the shared complex scratch (512 × 16 B = 8 kB), the window (4 kB) and the bin floors
    (257 × 8 B ≈ 2 kB) ≈ **34 kB** per stereo instance. The brief's "8 kB" was ~4× low and the doc
    comment carries this arithmetic.
35. **The per-frame contract.** `process_frame` (audio.rs:930) is **one interleaved sample frame**,
    mutated in place. The denoiser is the workspace's first block algorithm on that contract and it
    follows AU2's precedent exactly: the analysis runs **ahead** of a delay line on the signal path
    so the node's total delay is exactly `latency_frames` whatever the block clock does
    (`CompressorState.delay` audio.rs:737, `TruePeakLimiterState.delay` :768, :851-857).
36. **Per frame (R39, R49)**, in this order: write the input frame into `ring` at `cursor`; push the
    same frame into `bypass_delay` **unconditionally**, so the identity path is always primed (rule
    41); increment `since_block`, and when `since_block == hop` run one block (rule 37) and reset it
    to 0 — the block clock runs on **every** frame including `direct` ones, so the grid stays anchored
    at project sample 0 in every path and a switch never re-phases it. Then emit: if `direct` **or**
    `direct_switch_remaining > 0`, pop and **discard** one frame from the accumulator, decrement
    `direct_switch_remaining` when it is non-zero, and emit `bypass_delay`'s output; otherwise pop one
    frame from the accumulator, divide by 1.5, and emit it through `output_pad`. The `direct` branch
    therefore costs one transform per `hop` frames that it throws away — which is what buys the
    anchored grid and the dip-free switch, and is still 4× cheaper than nothing would be safe. R5
    refused to skip block work during preroll because it would desynchronise the OLA state; this is
    the same hazard and gets the same answer.
37. **Per block, per channel:** copy the newest `window` frames out of the ring in order, multiply by
    the Hann window into `scratch`, `forward_fft`; for each of the `window / 2 + 1` one-sided bins,
    with magnitude `m = |X_k|` and floor magnitude `f_k = sqrt(bin_floor_power[k])` (§3.3):

    ```
    a       = 10^(floor_offset_tenth_db / 200)          // amplitude ratio
    g_floor = 10^(-reduction_tenth_db / 200)            // amplitude ratio
    g_raw   = clamp((m - a * f_k) / m.max(f64::EPSILON), g_floor, 1.0)
    g_k     = smoother[k] + alpha * (g_raw - smoother[k])
    ```

    where `alpha = 1 - exp(-hop / (smoothing_milliseconds * rate / 1000))`, and `alpha = 1` when
    `smoothing_milliseconds == 0`. Mirror the gains onto the negative-frequency bins so the spectrum
    stays conjugate-symmetric and the inverse is real, `inverse_fft`, multiply by the Hann window
    again, rotate the accumulator by `hop` (zeroing the vacated tail) and add. `minimum_gain` takes
    `min` over `g_k` across the block and every channel, converted to `f32`, and is published by
    `take_gain_reduction` (§4.4).
38. **`reduction_tenth_db = 0` is an identity by construction**, not by luck: `g_floor = 10^0 = 1`,
    so the clamp is `clamp(_, 1.0, 1.0) = 1.0` for every bin. Rule 41 short-circuits it anyway.
39. **Where the two paths share code.** Nowhere new: one arm of `process_frame`, three existing call
    sites (export.rs:1276, export.rs:1300, audio.rs:3195), and rule 28's preroll anchoring the block
    grid at project sample 0 in live and export alike, seek or no seek. Bit-identity is therefore
    **structural**, not lucky (§3.10).
40. **Chunk boundaries do not quantise anything.** `MIX_CHUNK_SAMPLE_FRAMES = 1_024` (audio.rs:33)
    and export's literal 1 024 (export.rs:1247) never touch the node, which already runs per frame
    inside a chunk and owns its own block phase across chunks because the processor owns the runtime.
41. **The `direct` branch and its switch rule (R49, R40, R4).** `direct` is re-derived at construction
    and on every `parameter_epoch` bump as `bypass == 1 || reduction_tenth_db == 0 || every profile
    band == PROFILE_BAND_NEUTRAL_TENTH_DB`; `bypass` is additionally re-read per frame through the
    existing `(project_at, parameter_epoch)` cache (audio.rs:898-911), being hold-only and keyable.
    While `direct` is set the node still writes its ring, still runs its block clock (rule 36), runs
    no transform, and emits `bypass_delay.process(frame)` — a `DelayLine(latency_frames)` — so the
    identity is **bit-exact** and the declared delay is unchanged. This **narrows** AU2's bypass rule
    (audio.rs:941-947: "every dynamics node keeps its detector, envelope, window and release state
    running, so un-bypassing cannot emit `Lh` frames of material the gain computer never analysed")
    for this one node, and is recorded as an AU2 §0 erratum beside R6's (R40): a bypassed denoiser
    keeps its **ring** and its **block clock** running but not its transform, and
    `direct_switch_remaining` is what discharges AU2's obligation instead — the frames a fresh OLA has
    not yet analysed are emitted from the delay line rather than from a half-full accumulator. The
    deviation is defensible — the STFT is 25× the cost of a detector, which is why R5 exists — but it
    is a deviation, and it is recorded as one rather than claimed as conformance. When `direct`
    **clears**, the
    accumulator and smoother are zeroed and `direct_switch_remaining` is set to `window − 1` (511),
    during which the node keeps emitting from `bypass_delay`; 511 ≥ `window − hop` (384), the frames
    the accumulator needs to reach full overlap, so the switch can never dip. When `direct` **sets**
    the switch is immediate and exact, because `bypass_delay` has been fed all along. A Part A test
    toggles `bypass` mid-buffer through `process_buffer_static` and asserts the level never dips
    below the input's (A5).
42. *Rejected: the time-domain third-octave gate* (N0.3(b)'s second arm). A parallel bank of 31
    band-passes does not reconstruct — the phase sum is not the input — so a gate has to be a
    **serial cascade of 31 negative-gain `peaking` sections per channel** driven by a **second,
    parallel bank of 31 detection band-passes**: 124 biquads for stereo, the same order of arithmetic
    as the FFT, plus a `band_pass` constructor `dsp.rs` does not have, plus a coefficient-update
    cadence and its own smoothing to stop the cascade clicking when 31 gains move. More code than the
    STFT, its own block clock anyway, and it repairs strictly less: a band gate attenuates the signal
    living in the band along with the noise, which is audible pumping on speech. Named here as the
    fallback and nowhere else.

### 3.3 The profile's wire unit and the per-bin conversion (R3)

43. **Normative, the wire unit.** A profile band is **the band level `band_level_hundredths`
    (spectrum.rs:217-248) reports, carried in tenth-dB**: the overlap-weighted power of every bin in
    the band, normalised so a full-scale sine reads **0.00 dBFS** (`level = 10·log10(band_power / 0.5)`,
    spectrum.rs:245-247). Nothing else. This is a **wire format**, so it is defined here once and no
    other file may re-derive it.

    **Three conversions, normatively, and nowhere else (R34).** (i) `band_level_hundredths` answers
    `Option<i32>`; a band that answers `None` — no bins, or band power under
    `SILENCE_POWER = 1e-12` (spectrum.rs:36) — is written as **`PROFILE_BAND_NEUTRAL_TENTH_DB`
    (−1200)**, the neutral, so an unlearnable band gates nothing and a learn over digital silence
    yields the all-neutral profile rule 41 already handles. (ii) Hundredths become tenths by
    **`i32::div_euclid(10)` on the rounded hundredths figure**, i.e. round toward negative infinity,
    so a band is never written quieter than it measured and the conversion is one expression with one
    answer on every OS. (iii) The result is **clamped into `-1200..=0`**, the descriptor domain, so a
    band louder than a full-scale sine writes 0 rather than failing `SetEffectParam`
    (operation.rs:4142-4148) inside a prepared plan. A Part A test drives all three (A4).
44. **Normative, the runtime conversion.** For runtime bin `k` at centre frequency `f_k`:

    ```
    L_k          = linear interpolation, in tenth-dB, of the 31 band values
                   against log10(f) at the 31 ISO centres; held flat below the
                   first centre and above the last
    bins_in_band = max(1, count of one-sided runtime bins whose centre falls in
                   the third-octave band containing f_k, edges f_c · 2^(±1/6))
    bin_floor_power[k] = 10^(L_k / 100) * 0.5 / bins_in_band * (window² / 4)
    ```

    **The derivation, once.** `10^(L/100)` is the band's mean-square power relative to a full-scale
    sine's, `× 0.5` converts it to absolute mean-square power `P_band` (a full-scale sine's
    mean square is 0.5, which is exactly the normalisation of rule 43). Dividing by `bins_in_band`
    spreads that power evenly over the band's one-sided runtime bins. The `window² / 4` factor
    converts a time-domain mean-square figure into the squared magnitude the **unnormalised**,
    Hann-windowed, `window`-point `forward_fft` actually produces: by Parseval,
    `Σ_k |X_k|² = N · Σ_n |w_n x_n|²`, and a periodic Hann has `Σ_n w_n² = N/2`, so a signal of mean
    square `P` spread over the whole spectrum gives `Σ_k |X_k|² = N²P/2` two-sided, i.e. `N²P/4` over
    the one-sided half. Sanity check: one band covering all `N/2` bins at 0 dB gives
    `0.5/(N/2) × N²/4 = N/4` per bin, and the direct figure `E|X_k|² = N·P_band/2 = N/4` agrees.
45. `bins_in_band` is computed **once at construction**, not per block. At 512/48 kHz the bin spacing
    is 93.75 Hz, so the low bands hold zero bins and the `max(1, …)` floor keeps the conversion
    finite; the top band holds ≈ 49. Without the `/ bins_in_band` term the floor would be overstated
    by `10·log10(bins_in_band)` — up to ≈ 17 dB of over-gating at the top of the spectrum — which is
    why R3 called the missing unit a document-compatibility break rather than a bug. The mismatch
    between the 4 096-point measurement window and the 512-point runtime window is absorbed entirely
    by rule 44, because a band **level** is window-independent by construction; the measurement
    window stays **4 096 with a 2 048 hop** (§3.7).
46. **Pinned by a band-limited-noise fixture (R3).** Noise band-limited to the 1 kHz third-octave band
    at a known level L reads back through `mix_noise_profile` within
    `DENOISE_PROFILE_LEARN_BUDGET_TENTH_DB = 15` of L, and with that profile on the node at
    `reduction_tenth_db = 400` the residual in the band reads within
    `DENOISE_PROFILE_GATE_BUDGET_TENTH_DB = 30` of `L − 40 dB`. Both terms print their margins (A4).

### 3.4 `audio_hum_removal`: the peaking cascade (D8, R28)

47. **Normative.** For `h in 1..=harmonic_count`, one `BiquadSection` at `h × fundamental_hertz` with
    `gain_db = depth_tenth_db / 10.0` and `q = notch_q_hundredths / 100.0`, built by the existing
    `BiquadCoefficients::peaking(hertz, gain_db, q, rate)` (dsp.rs:114-128); a section whose centre
    reaches `h × f >= 0.45 × rate` is `IDENTITY` and skipped. **No new `BiquadCoefficients`
    constructor.** A true notch is infinitely deep and rings; an editor wants −30 dB, not −∞, and
    depth is the control they reach for. Negative-gain `peaking` gives exactly that, is validated to
    Q 18 by `audio_parametric_eq`'s `q_hundredths` domain (effect.rs:1869), and — decisively —
    `magnitude_db` (dsp.rs:134-147) already exists for it, so the app well and the response tests
    compare against the **design** rather than against a second implementation.
48. **Ten sections are allocated at construction**, from the descriptor's `harmonic_count` **maximum**
    (rule 11), the ones past the stored count carrying `IDENTITY`. `HumState` holds
    `sections: Vec<[BiquadSection; HUM_SECTIONS]>` per channel plus `cached: Option<(TimeCode, u64)>`;
    coefficients are recomputed on the existing `(project_at, parameter_epoch)` cache, the
    parametric-EQ idiom (audio.rs:1000-1010).
49. **`depth_tenth_db = 0` is bit-exact identity.** RBJ peaking at `A = 1` gives `b0 = a0 = 1 + α`,
    `b1 = a1 = −2cos ω`, `b2 = a2 = 1 − α`, so numerator and denominator are bitwise equal, `b0/a0`
    is exactly `1.0`, and the difference equation cancels term for term from a zeroed state;
    `STATE_SQUELCH = 1e-30` (dsp.rs:170-215) only flushes denormals.
50. `audio_hum_removal` declares **no lookahead at all** — the descriptor has no
    `lookahead_milliseconds` row — so `chain_lookahead_milliseconds` contributes 0 for it and §3.6's
    per-node read must contribute 0 too. That is the sharpest arm of the B1 test (A2).
51. **The analytic response helper.** `pub fn hum_removal_magnitude_db(effect: &Effect, at: TimeCode,
    hertz: f64, sample_rate: u32) -> f64` lands beside `parametric_eq_magnitude_db`
    (audio.rs:707-722), summing `magnitude_db` over rule 47's sections. It draws the Mixer's comb
    (§6.2) and is what §3.11(b)'s analytic arm compares the measured tone response against, on AU2's
    "compare against the design" rule. **Fixed fundamental, no tracking** (§1.3): 50 or 60 Hz, chosen
    by the editor or by `get_audio_repair`'s two measured excesses (§3.9).

### 3.5 `audio_declick`: detection, guard and repair (D9, R2, R7, R30)

52. **Normative, the detector.** `d2[n] = x[n] − 2·x[n−1] + x[n−2]`, per channel, computed on the
    **undelayed** input. A sample is flagged when
    `|d2[n]| > 10^(detector_threshold_tenth_db / 200) × rms_reference(n)`.
53. **Normative, the reference (R7).** `rms_reference(n)` is a **strictly causal trailing** mean
    square over `DECLICK_REFERENCE_MILLISECONDS = 20` of `d2`, ending at `n − 1`, that **excludes
    flagged samples**: a flagged sample contributes neither to the sum nor to the divisor, and the
    reference holds its last value while fewer than a quarter of the window's samples are unflagged.
    Without the exclusion the detector is self-masking on its own fixture, and the arithmetic that
    proves it lives in the doc comment beside `DECLICK_ERROR_DROP_BUDGET_TENTH_DB`:

    > §3.11(c)'s click writes 8 frames of ±0.9, whose `d2` reaches 4 × 0.9 = **3.6**, which is the
    > second difference of a **sign-alternating** run; §3.11(c) writes the click that way for exactly
    > this reason (R31). The 440 Hz
    > carrier's `d2` is `A·(2 sin(ω/2))² ≈ A·ω² = 0.3 × 0.05760² = 9.95e-4` peak, **7.04e-4** RMS. At
    > `detector_threshold_tenth_db = 240` (24 dB = ×15.85) the threshold is **1.12e-2** and the click
    > clears it by 322×. If the reference window were allowed to contain the click, its mean square
    > would be `8 × 3.6² / 960 = 0.108` → RMS 0.329 → threshold **5.21 > 3.6**, and the node would
    > detect **nothing**. The exclusion is the whole detector.
54. **Runs widen to spans**, and a span longer than
    `stage_latency_frames(max_click_milliseconds, rate)` is **not repaired**: that is a dropout, not a
    click, and §1.3 defers it. **The false-positive rule (R2, R30):** a span is repaired only when the
    `d2` energy in the **1 ms** before *and* after it is itself below threshold, so a snare or a
    plosive — which raises `d2` for tens of milliseconds — never qualifies, while a click is by
    definition isolated. The guard is 1 ms, not 2, because 2 ms does not fit the declaration.
55. **Normative, the construction assert (R2).** At construction, over the **whole domain** of
    `max_click_milliseconds` (`0..=1`) and at 44 100, 48 000 and 96 000 Hz:

    ```
    stage_latency_frames(DECLICK_DECLARED_MILLISECONDS, rate)
        >= stage_latency_frames(max_click, rate)
         + stage_latency_frames(DECLICK_GUARD_MILLISECONDS, rate)
         + DECLICK_DETECTOR_FRAMES
    ```

    | rate | declared (3 ms) | max_click 1 ms | guard 1 ms | `d2` delay | required | fits |
    | --- | --- | --- | --- | --- | --- | --- |
    | 44 100 | 132 | 44 | 44 | 2 | 90 | ✓ |
    | 48 000 | 144 | 48 | 48 | 2 | 98 | ✓ |
    | 96 000 | 288 | 96 | 96 | 2 | 194 | ✓ |

    The brief's original figures — `max_click` 2 ms and a 2 ms guard — needed 194 frames against 144
    at 48 kHz and 178 against 132 at 44.1 kHz, which is why the ceiling is 1 ms. **The declaration
    does not rise to 5 ms**: §2.3's normalization row would become 12 + 5 + 5 = 22 > 20 and the budget
    argument would collapse. The assert is a real `assert!` in `AudioEffectRuntime::new`, not a
    `debug_assert`, and it is also a standalone Part A test over the three rates so a release build
    cannot hide a regression (A6).
56. **Normative, the topology.** The signal path is one `DelayLine(latency_frames)`; the detector
    reads the **undelayed** input and a repair writes **into the delay line's buffer** at the span's
    position. When nothing is repaired the output is the delayed input **bit for bit**, which is what
    makes `max_click_milliseconds = 0` an identity (rule 4). **Repair is Catmull-Rom** in `f32` from
    the two good samples either side, chosen over linear because its residual over an `H`-frame span
    of a band-limited signal is `A(ωH)⁴/384`, four orders below linear's `A(ωH)²/8`.
57. **Detect-only mode (R41).** The same detector *and the same rule 54 acceptance test* — the
    length ceiling **and** the 1 ms pre/post `d2` guard — factored as
    `pub(crate) fn detect_clicks(samples: &[f32], channels: u16, rate: u32, threshold_tenth_db: i64,
    max_click_milliseconds: i64) -> ClickReport`, is where §3.9's `click_count` and
    `click_density_per_minute` come from. A `ClickReport` counts exactly the spans the node **would
    repair**, never the spans it merely flagged, so "what is a click" cannot drift between the node
    and the inspector and §3.11(c)'s transient arm reads zero for the reason it is written to read
    zero: it is the **guard**, not the length test, that rejects a 5 ms burst's short attack run.

### 3.6 `node_lookahead_milliseconds` reads the descriptor (B1, R1)

58. **Normative.** `node_lookahead_milliseconds` (audio.rs:1361-1373) becomes:

    ```rust
    /// AU5 §3.6: one node's declared latency, read exactly as core reads it.
    ///
    /// The neutral comes from the descriptor, never from a literal here: core's
    /// `chain_lookahead_milliseconds` (model.rs:735-755) falls back to
    /// `effect_descriptor(name).parameter(LOOKAHEAD).neutral`, and the two
    /// agreed before AU5 only by coincidence — compressor neutral 0, limiter
    /// neutral 5 — while `audio_denoise`'s is 12.
    fn node_lookahead_milliseconds(effect: &Effect) -> i64 {
        if !is_static_audio_parameter(&effect.name, AUDIO_LOOKAHEAD_PARAMETER) {
            return 0;
        }
        let neutral = effect_descriptor(&effect.name)
            .and_then(|descriptor| descriptor.parameter(AUDIO_LOOKAHEAD_PARAMETER))
            .map_or(0, |parameter| parameter.neutral);
        static_audio_value(effect, AUDIO_LOOKAHEAD_PARAMETER, neutral)
    }
    ```
59. **Why this is a blocker and not a tidy-up.** N0 requires defaults to be omitted on the wire and
    the agent writes `Effect.parameters` freely, so an `audio_denoise` whose
    `lookahead_milliseconds` is simply absent would have yielded `latency_frames = 0` in media —
    `window =` the largest power of two `≤ 0`, a degenerate or panicking construction, a bus pad
    that mis-sums, and a `process_buffer_static` length change — while **core** declared 12 ms and
    compensated for it. It is silent, rate-dependent, and it corrupts both paths identically so
    §3.10's parity tests would stay green. It lands in Part A **before the first denoise node
    exists**.
60. `node_structure` (audio.rs:1345-1358) reads the same function and is otherwise unchanged: it
    stays `(EffectId, &name, static lookahead)`. The 31 profile bands are deliberately **not** in it,
    which is what lets a `Learn` bump `parameter_epoch` and preserve the OLA accumulator, the filter
    memories and the delay lines (rule 73).

### 3.7 `Analysis::mix_noise_profile` (D6, R9, R10, R27)

61. **Normative.** The profile is measured **synchronously through the mix path**, not cached per
    asset.

    ```rust
    /// AU5 §3.7: learn a noise floor from one project range at one point.
    ///
    /// # Errors
    /// `NotImplemented` by default; a range under `NOISE_PROFILE_MINIMUM_FRAMES`.
    fn mix_noise_profile(&self, document: &Document, request: &MixNoiseProfileRequest)
        -> Result<NoiseProfileReport, MediaError> { Err(MediaError::NotImplemented) }

    struct MixNoiseProfileRequest { range: Option<Range<TimeCode>>, point: MixSpectrumPoint }
    struct NoiseProfileReport {
        range: Range<TimeCode>, point: MixSpectrumPoint,
        sample_rate: u32, sample_frames: u64, windows: u32,
        bands: [i32; 31],   // low to high, tenth-dB on rule 43's scale
    }
    ```

    It is modelled on `mix_spectrum` (media.rs:2051, `measure_mix_spectrum` export.rs:1746) and
    differs in **three** things, all of them named here (R33). (i) `third_octave_spectrum`
    (spectrum.rs:126-205) is **parameterised** by `(segment_frames, hop_frames, minimum_frames)`;
    AU2's callers pass `SPECTRUM_SEGMENT_FRAMES / SPECTRUM_HOP_FRAMES / SPECTRUM_MINIMUM_FRAMES` and
    the goldens are pinned byte-unchanged (A4), while the profile passes
    `NOISE_PROFILE_SEGMENT_FRAMES = 4_096`, `NOISE_PROFILE_HOP_FRAMES = 2_048` and
    `NOISE_PROFILE_MINIMUM_FRAMES = 22_528`. Reusing the routine unparameterised is not an option:
    `SPECTRUM_MINIMUM_FRAMES` is 24 576 and would refuse the ruled ten-window minimum. (ii) The
    reduction over windows is the **20th-percentile** band level rather than a Welch mean, taken with
    `nearest_rank_index`. (iii) At a 4 096 window the Hann main lobe is 46.9 Hz, so **eleven** bands
    (20 Hz … 200 Hz) are `window_limited` rather than five; `NoiseProfileReport` reports them as
    learned, and §3.3 rule 44's interpolation is what smooths them, but the doc comment says so and
    §3.11(a′)'s unit pin is measured in the **1 kHz** band deliberately, not at the bottom.

    The parameterisation threads through five sites and is named in §8 before AU2's `mix_spectrum`
    goldens are touched: `hann_window`, the `power` vector, the `scratch` buffer, `normalisation`
    (spectrum.rs:145-151) and `normalised_length()` (:210-213) all read the module constants today.
    The remaining constants are `NOISE_PROFILE_PERCENT = 20` and
    `NOISE_PROFILE_MINIMUM_FRAMES = SEGMENT + 9 × HOP = 22_528` — **469.3 ms at 48 kHz, exactly ten
    windows**, the fewest a percentile means anything over. `nearest_rank_index` is
    loudness.rs:219-222, `pub(crate)` per R21.
62. **Why the mix path and not a fifth `AnalysisKind` (R9, R27).** (i) The profile must describe what
    the **node** sees — post track gain, pan and routing, pre-chain — which an asset-domain analysis
    structurally cannot. (ii) The machinery exists: `mix_audio_stems` renders a stem over a project
    range and `spectrum.rs` folds bins to the same 31 bands, so the profile is `mix_spectrum` with a
    different reduction. (iii) **What N0.3(c) really costs is the cache**, not the asynchrony: the
    single-flight worker is paid anyway by §6.3's `Learn` button and already exists
    (color_qc_ui.rs:608-623, :682, :929-954, app.rs:1000-1002). The cache is a fifth `AnalysisKind`
    and every exhaustive match over `ALL: [Self; 4]`, a status enum, a `request_*`/`*_status` pair
    with a **range** argument no existing request has, a new `JsonCache` config-key shape, a
    `BaselineProofAnalysis` arm guarded only by a hand-written test, and an arm in app.rs:1258-1266's
    single 100 ms predicate — to cache a measurement that takes milliseconds. (iv) N0.3(c)'s real
    invariant, "never document state beyond an id/digest", is honoured **better** by §2.1, because
    the 31 integers *are* the result.
63. **Cost accepted and stated:** a range shorter than 469 ms cannot be learned from, and that is a
    refusal both callers spell in words (§5.6, §6.3), never a silent zero.

    **The three domains, once (R37).** `NOISE_PROFILE_MINIMUM_FRAMES` is **audio sample frames** at
    the render rate (48 000, export.rs:37-38). The minimum handed to `Analysis::timeline_silences` is
    **source frames** on the asset's own grid — an audio-only asset probes at `Rational::default()` =
    30/1 (decode.rs:105-112) — so it is
    `NOISE_PROFILE_MINIMUM_SOURCE_FRAMES = ceil(22 528 / 48 000 × source_fps)`, computed per asset,
    never a literal. The learn **range** is **project frames**:
    `ceil(22 528 / 48 000 × project_fps)`, and `mix_noise_profile` re-checks the rendered sample-frame
    count itself and refuses with `MixLoudnessRangeTooShort` rather than trusting the caller's
    conversion (rule 69). The editor-facing sentence stays "No silence span reaches 469 ms" in all
    three cases, because milliseconds is the only unit an editor has.
64. The range for a `Learn` comes from `Analysis::timeline_silences` over the tracks feeding the
    chain — **behind R10's readiness gate**, because `timeline_silences` (media.rs:1992-1997) is fed
    by a per-asset async analysis and returns `Ok(vec![])` on an unanalysed project. The routine takes
    `(document, range, minimum_source_frames)` and has **no track filter** (R46): the **caller**
    filters the answer on `TimelineSilenceSpan.track` (media.rs:1489), exactly as R13's gap planner
    filters to audio itself.

### 3.8 `Analysis::mix_window_levels` — the short-window RMS accessor AU4 deferred (D7, F19)

65. **Normative.**

    ```rust
    /// AU5 §3.8: short-window RMS levels over one project range at one point.
    ///
    /// Deliberately **not** BS.1770 — no 400 ms gating block, no K-weighting —
    /// which is exactly why `plan_clip_fades` wanted it (server.rs:8776-8783).
    fn mix_window_levels(&self, document: &Document, request: &MixWindowRequest)
        -> Result<MixWindowLevelReport, MediaError> { Err(MediaError::NotImplemented) }

    struct MixWindowRequest {
        range: Option<Range<TimeCode>>, point: MixSpectrumPoint,
        window_milliseconds: u32,  // 1..=1000, rejected outside it by name
        hop_milliseconds: u32,     // 1..=window_milliseconds, likewise
    }
    struct MixWindowLevelReport {
        range: Range<TimeCode>, point: MixSpectrumPoint, sample_rate: u32,
        window_milliseconds: u32, hop_milliseconds: u32,
        windows: Vec<Option<i32>>, // dBFS hundredths; `None` under `SILENCE_POWER`
    }
    ```
66. The window and hop are converted with `stage_latency_frames`, the same truncating helper the
    nodes use (audio.rs:581-586), so a window is an exact frame count at every rate.
67. **AU4's debt is discharged in Part A.** `plan_clip_fades`' two-render head/tail hack
    (server.rs:8765-8790) becomes one `mix_window_levels` call, the `MixLoudnessRangeTooShort`
    skip-with-a-reason arm for a clip shorter than one gating block goes away, and the AU4 comment
    naming AU5 (server.rs:8768-8774) is **deleted**. Its existing tests must stay green with the same
    proposed fades on the same fixtures, which is the regression that proves the accessor agrees with
    the thing it replaces (A17).

### 3.9 `Analysis::audio_repair` (D13, R18)

68. **Normative.** `fn audio_repair(&self, document: &Document, request: &AudioRepairRequest) ->
    Result<AudioRepairReport, MediaError>`, defaulting to `NotImplemented`, measured by
    `measure_audio_repair` in `export.rs` beside `measure_audio_qc` (:1689-1712) and
    `measure_mix_spectrum` (:1746-1794), through one `mix_pass` with one observer. Three measurements
    over one render:
    - **The percentiles.** `REPAIR_WINDOW_MILLISECONDS = 10` RMS windows, hop = window, over the
      chosen point; the 10th and 90th percentile window levels by `nearest_rank_index`; `snr` is
      their difference (§2.4 rule 21). Windows below `SILENCE_POWER` are excluded from the
      percentile population and counted into `windows`.
    - **Hum excess (R18).** A Goertzel at 50 and 60 Hz **and their first three harmonics** — no FFT,
      no new dependency — each over `HUM_GOERTZEL_BLOCK_FRAMES = 4_800` samples (100 ms at 48 kHz,
      enough to resolve 50 from 60 Hz), averaged over as many whole blocks as the range holds. Each
      harmonic's excess is its level minus the **mean of its two sixth-octave shoulders** at
      `f × 2^(±1/6)`, two further Goertzels at the same block length. `hum_50_excess_db_hundredths`
      is the **sum** over the four harmonics of `max(0, excess)`, with the four values also reported
      separately; likewise for 60 Hz. Both are `None` under one whole block.
    - **Clicks.** §3.5 rule 57's `detect_clicks` in detect-only mode at the descriptor's neutral
      threshold, over the same rendered stem. `click_density_per_minute` is
      `click_count × 60 × sample_rate / sample_frames`, integer, rounded down.
69. `MediaError` gains **no** new variant: a short range reuses the existing
    `MixLoudnessRangeTooShort` spelling for `mix_window_levels`, and
    `mix_noise_profile`'s own floor is reported as `MixLoudnessRangeTooShort` with the noise-profile
    minimum interpolated, exactly as `get_audio_levels` re-spells it today (server.rs:7227-7237).

### 3.10 Numeric discipline and both-paths parity (§4 of the brief)

70. **Integer on every wire.** Profile bands tenth-dB; SNR, noise floor, signal and hum excess
    hundredths dB; window levels hundredths dBFS; click counts `u32`; every range a `TimeCode` pair.
    **No `f32`/`f64` leaf on any AU5 serialized type**, so all of them derive `Eq`. **f64 inside the
    denoiser, f32 at its edges**: the scratch, window, transform, bin gains and OLA accumulator are
    `f64`, because `forward_fft` and `hann_window` are and because the accumulator sums four
    overlapping windows per output sample; the node reads and writes `f32` frames like every other
    node. Hum stays in `BiquadSection` (f64 coefficients, f32 state) and de-click in f32 — no change
    to AU2's numeric habits.
71. **Latency is exact, not approximate, and it is pinned by an impulse (R19).** For each node,
    `internal delay + DelayLine pad == stage_latency_frames(declared_ms, rate)`, asserted at
    construction and **proved** by pushing a unit impulse through `process_buffer_static`
    (audio.rs:1305-1342) and asserting the output impulse lands at frame 0 after the helper's head
    trim — not by re-deriving the arithmetic in the test, which would only re-assert the code.
    `process_buffer_static` **errors rather than panics** on a length change, so a node whose delay
    drifts fails its own unit test.

    **The impulse pins the declared delay only (R32).** Because rule 41 puts an all-neutral
    `audio_denoise` on the `direct` branch, an impulse through a bare node exercises the delay line
    and never the OLA, and there is no parameter setting that both leaves `direct` and reconstructs
    bit-cleanly. The **OLA delay is therefore pinned separately and directly**: a unit test constructs
    `DenoiseState` at each rate, forces `g_k ≡ 1.0` for every bin and every block, pushes
    `4 × window` frames of `pseudo_random_amplitude`, and asserts the output equals the input delayed
    by exactly `window − 1` frames to within **1e-12** over the fully-overlapped region
    `[window − hop, …)`. Hann²/1.5 reconstruction is exact only up to rounding, which is why this arm
    carries a tolerance and the `direct` branch exists (R49). A second arm asserts
    `output_pad.len() == latency − (window − 1)` equals 65 / 18 / 129 at 48 / 44.1 / 96 kHz — the one
    place the arithmetic is restated, because the impulse cannot reach it.
72. **Neutrality is bit-exact**, and both paths are one implementation. Every neutral is an identity
    (rule 4); a chain with no repair node runs the pre-AU5 code path unchanged; a pre-AU5 document
    exports **byte-identically**, pinned with the AU3 `to_bits()` + printed-SHA-256 idiom
    (`AU5_REPAIR_SHA256`). One `process_frame` implementation is reached from export.rs:1276,
    export.rs:1300 and audio.rs:3195, and rule 28's preroll anchors the STFT block grid, the notch
    state and the click detector at project sample 0 in live and export alike, seek or no seek — so
    bit-identity is **structural**, not lucky. Pinned two ways: `to_bits()` equality of `mix_audio`
    run twice, and `assert_playback_matches_export` (audio.rs:4896-4940) at **1e-6** on a
    repair-bearing fixture **including its frame-5 seek arm**. Export never ramps: no `GainRamp` is
    involved. **Two** pieces of state can differ between a fresh runtime and a live-retargeted one,
    and both are bounded and named (R38): the per-bin gain smoother, which starts from the first
    block's own gains identically in both; and rule 41's `direct_switch_remaining` window, which is
    up to `window − 1` frames of delay-line output after a `Learn` clears `direct`. Neither is
    reachable in the export path, which always builds fresh from the document at project sample 0, so
    A14's 1e-6 both-paths pin is asserted on a chain that is **not** retargeted mid-stream, and the
    retarget case is pinned separately by A14's "identical bin floor table after a retarget that
    preserves state" arm plus a new arm asserting the switch window is exactly `window − 1` frames
    long and never dips.
73. **The live retarget rule.** `retarget_chain` matches on `node_structure` (rule 60), so a `Learn`
    — which changes only profile bands — bumps `parameter_epoch` and **preserves** the OLA
    accumulator, the filter memories and the delay lines; the runtime re-derives its bin floor table
    (rule 44), its notch coefficients (rule 48) and its `direct` flag (rule 41) **on epoch change**.
    That is the one place a static parameter is re-read after construction, and rule 9 is where it is
    written down. Inserting or removing a repair node changes the declared lookahead, which
    `live_audio_change` (app.rs:1767-1771) already forces to `None` — a re-cue, correct, because the
    graph latency moved. No app change.
74. **Determinism.** `forward_fft` calls `sin_cos` per butterfly today and reads rule 28's table
    after AU5; within one process that is one function with one answer, so the three call sites agree
    bit for bit. Cross-**OS** byte identity of an *encoded export* is not claimed and never was —
    AU3's cross-OS gates are LU/dBTP budgets with margins, and N0's rule is about tolerances, not
    byte equality. **The 1e-4 seam** is a sample-domain tolerance and lives in **media**, beside
    AU4's both-paths tests; core is integer-only and has no home for it (§5.4).

### 3.11 Fixtures and budgets (exit-gate clause 1; R8, R19, R20, R21, R31, R45, R46)

New `crates/kinewright-media/tests/au5_fixtures.rs` under `test-util`, following
`tests/au3_fixtures.rs` (:9-15, :30-59). Every fixture is **exact `wav_f32` bytes** through
`GeneratedMedia::from_bytes` — no lavfi, because the provisioned FFmpeg's `sine` emits −18 dBFS
(au3_fixtures.rs:127-131).

**The helper signatures, once, and unchanged (R45).**
`tone(frequency: f64, amplitude: f32, rate: u32, frames: usize) -> Vec<f32>` (audio.rs:5337-5346) is
**mono** and returns a bare sample vector; `pseudo_random_amplitude(samples: usize, amplitude: f32)
-> Vec<f32>` (audio.rs:5350-5361) is flat and its first argument is a **sample** count, not a frame
count. R21 promotes both **with their signatures unchanged** — changing them would break
`pseudo_random` (audio.rs:5363-5365) and every existing AU2/AU3 unit test — so every fixture below
sums mono vectors and **interleaves to stereo itself**, and every call is spelled in full.
`pseudo_random_amplitude` is xorshift64 on a fixed integer seed, uniform on `[−a, a)` in exact binary
steps and identical on every OS, so its RMS is `a/√3`. All fixtures are 48 kHz stereo; named budget
constants carry their derivation in the doc comment, `FIXTURE_MINIMUM_MARGIN = 2.0`, one printed line
per lane, **no `cfg`-conditioned tolerance**.

**(a) Broadband noise (R45).** 3 s: `[0, 1 s)` noise only, `[1, 2.5 s)` tone + noise, `[2.5, 3 s)`
noise only. `noise = pseudo_random_amplitude(144_000, 0.010)` — 3 s of **mono samples** — → RMS
`0.010/√3 = 5.774e-3` = **−44.77 dBFS**; `tone = tone(1_000.0, 0.200, 48_000, 72_000)`, summed into
`noise` at mono sample 48 000 → RMS `0.200/√2 = 0.14142` = **−16.99 dBFS**; the sum is interleaved to
stereo. Learn the profile from `[0, 1 s)` — 48 000 frames ≥ `NOISE_PROFILE_MINIMUM_FRAMES` 22 528 ✓
— then set
`reduction_tenth_db = 200`, `floor_offset_tenth_db = 0`, and measure the tail `[2.5, 3 s)` and the
tone window `[1.2, 2.3 s)`.

| term | expected | budget const | gate | margin |
| --- | --- | --- | --- | --- |
| noise-floor drop, tail RMS before − after | ≈ **19.0 dB** (a 20 dB request reaches within ~1 dB in noise-only material) | `DENOISE_FLOOR_DROP_BUDGET_TENTH_DB = 90` | drop ≥ 9.0 dB | 2.1× |
| 1 kHz tone loss, Goertzel before − after | ≈ **0.2 dB** (the tone's bins sit far above the learned floor, so `g ≈ 1`) | `DENOISE_TONE_LOSS_BUDGET_TENTH_DB = 10` | loss ≤ 1.0 dB | 5× |

Print `AU5_DENOISE measured_drop_tenth_db=… measured_tone_loss_tenth_db=… margin_drop=… margin_tone=…`.

**(a′) The unit pin (R3).** A second lane carries noise band-limited to the 1 kHz third-octave band
at a known level L — 64 phase-randomised sinusoids inside the band, so L is analytic, not measured.
`mix_noise_profile` reads that band within `DENOISE_PROFILE_LEARN_BUDGET_TENTH_DB = 15`; with that
profile on the node at `reduction_tenth_db = 400` the residual reads within
`DENOISE_PROFILE_GATE_BUDGET_TENTH_DB = 30` of `L − 40 dB`, and every neighbouring band moves under
3.0 dB. Print `AU5_DENOISE_UNIT learned_tenth_db=… expected_tenth_db=… gated_tenth_db=… margin=…`.

**(b) Hum (R45).** 2 s, summed mono then interleaved to stereo:
`tone(300.0, 0.200, 48_000, 96_000)` + `tone(50.0, 0.0500, 48_000, 96_000)` +
`tone(100.0, 0.0250, 48_000, 96_000)` + `tone(150.0, 0.0125, 48_000, 96_000)`
— harmonics at −6 dB each. Hum RMS `= √((0.05² + 0.025² + 0.0125²)/2) = 0.04051` = **−27.85 dBFS**.
Node: `fundamental_hertz 50`, `harmonic_count 3`, `depth_tenth_db −300`, `notch_q_hundredths 1200`
(Q 12 → a 4.17 Hz bandwidth at 50 Hz).

| term | expected | budget const | gate | margin |
| --- | --- | --- | --- | --- |
| hum drop, Goertzel sum at 50/100/150 | ≈ **30 dB** — the **exactly-on-centre** case; a ±0.5 Hz mains drift in a 4.17 Hz bandwidth costs several dB, so this proves the design, not the field (R20) | `HUM_DROP_BUDGET_TENTH_DB = 140` | drop ≥ 14.0 dB | 2.1× |
| 300 Hz loss | ≈ **0.13 dB** (≈ 0.09 from the 150 Hz section, 0.03 from 100, 0.01 from 50 — R20 corrects the brief's 0.05) | `HUM_TONE_LOSS_BUDGET_TENTH_DB = 5` | loss ≤ 0.5 dB | 3.8× |

Plus one analytic arm: `hum_removal_magnitude_db` at 50/100/150/300 Hz matches the measured tone
response within **0.1 dB**, the AU2 "compare against the design" rule (rule 51).
Print `AU5_HUM measured_drop_tenth_db=… measured_tone_loss_tenth_db=… analytic_delta_db=… margin_drop=… margin_tone=…`.

**(c) Clicks (R8, R20, R31, R45, R46).** 2 s `clean = tone(440.0, 0.300, 48_000, 96_000)`,
interleaved to stereo (RMS 0.2121, −13.5 dBFS). `corrupt` sets 8 consecutive frames on both channels
at frame `4_800 × (k + 1)` for **`k in 0..19`** to `0.9 × (−1)^(k + j)` for `j in 0..8` — the sign
alternates **per frame** within the click, and `k` only fixes each click's starting polarity, so the
run is deterministic and every frame of it carries the full second difference. A constant 8-frame
step would be invisible to a `d2` detector in its middle six frames
(`d2 = +c, −c, 0, 0, 0, 0, 0, 0, −c, +c`) and is the wrong corruption for this node: it would widen
into **two** two-sample runs per click, read `click_count == 38`, leave the middle six frames of the
±0.9 plateau unrepaired and miss `DECLICK_ERROR_DROP_BUDGET_TENTH_DB` by ≈ 29 dB. **19** clicks,
100 ms apart, the last occupying frames **`91_200..91_208`** of 96 000 (the brief's `0..20` wrote one
frame past the end of the fixture). Node: `max_click_milliseconds 1` (48 frames ≥ 8),
`detector_threshold_tenth_db 240`, `lookahead_milliseconds 3`.

| term | expected | budget const | gate | margin |
| --- | --- | --- | --- | --- |
| error drop, **RMS-to-RMS**: `20·log10(rms(corrupt − clean) / rms(repaired − clean))` over the whole fixture | ≈ **88–91 dB**. Catmull-Rom's residual over an 8-frame span of a 440 Hz sine is `A(ωH)⁴/384 ≈ 3.5e-5` **peak** (R20: a peak bound, quoted against an RMS signal), which over 19 × 8 × 2 of 192 000 samples is ≈ 9.9e-7 RMS against ≈ 0.0368 before | `DECLICK_ERROR_DROP_BUDGET_TENTH_DB = 300` | drop ≥ 30 dB | ≈ 3× on the gate, and the gate is set low on purpose: the residual is dominated by detector edge handling, not by the interpolation bound |
| detection | `click_count == 19` on `corrupt`, `== 0` on `clean` | — | exact | — |
| transient control | a 5 ms exponential burst reports `click_count == 0` and is returned **unmodified within 1e-6** | — | exact | — |

`rms(corrupt − clean)` is printed rather than predicted (R20), and the assert is against the budget,
never the estimate. With the click sign alternating per frame (R31) the error is `±0.9 − x[n]`, so
`E[(±0.9 − x)²] = 0.81 + E[x²] = 0.81 + 0.045` and `rms(corrupt − clean) ≈ 0.0368` against the
analytic 0.0358 — a difference of **0.24 dB**, not the ~3 dB the draft quoted (R46). Print
`AU5_DECLICK measured_drop_tenth_db=… quantity=rms_to_rms before_rms=… after_rms=… clicks=… margin=…`.

**(d) The Part A SNR-gain lane (R16) — what makes Part A a complete exit gate on clause 1.** Fixture
(a) routed through a bus carrying one `audio_denoise`; `Analysis::audio_repair` on the bus point
before and after the profile is written and the reduction set to 200, with
`after.snr_db_hundredths − before.snr_db_hundredths` asserted against
`AUDIO_REPAIR_SNR_GAIN_BUDGET_HUNDREDTHS = 600`. Expected: the 10th percentile sits in the noise-only
thirds at −44.8 dBFS and the 90th in the tone third at −17.0, so the SNR before is ≈ 27.8 dB; after,
the floor falls ≈ 19 dB and the tone loses ≈ 0.2, so the gain is ≈ **18.8 dB** — margin ≈ 3.1×. Print
`AU5_REPAIR_SNR before_hundredths=… after_hundredths=… gain_hundredths=… margin=…`.

**(e) The Part B planner budget** (§5.6): `PLAN_REPAIR_SNR_GAIN_BUDGET_HUNDREDTHS = 300` against the
same fixture through `plan_dialogue_repair`'s **default** `reduction_tenth_db = 120`, where the
measured gain is ≈ 12 dB — margin 4×.

**Promotions (R21).** `tone`, `pseudo_random_amplitude`, `rms` and `wav_f32` move from `audio.rs`'s
`#[cfg(test)]` module to `crates/kinewright-media/src/test_support.rs`, staying
`#[cfg(any(test, feature = "test-util"))]`, so `tests/au5_fixtures.rs` reaches them by the `#[path]`
route `tests/au3_fixtures.rs:27` already uses.

### 3.12 Part A media evidence map

Exit-gate clause 1 is closed by §3.11 lanes (a), (a′), (b), (c) and (d) — checklist items A4 and
A7–A11 — plus the latency and identity pins of A5 and A12. Everything numeric is printed beside its
assert in the `AU3_NORMALIZE` / `CC6_LANE_MEASURED` house style, every gated term keeps ≥ 2× margin,
and no tolerance is OS-conditional.

---

## 4. Part A agent and app

### 4.1 `get_audio_repair` (D13, R18)

75. One inspector on `audio_qc_tool.rs`'s exact shape (293 lines): an `AUDIO_REPAIR_DESCRIPTION`
    const under M36's 1 024 B exclusive budget, `AudioRepairArgs` with `deny_unknown_fields` and an
    optional `expected_revision` resolving to the uniform `stale_revision` refusal, structured
    `{ timeline_revision, report }` plus rendered text.
76. **The first sentence carries the load (R18)**, because `get_capability` and
    `search_capabilities` publish only `first_sentence(description)` (runtime.rs:184-189):

    > "Measures a percentile noise floor, percentile SNR, 50/60 Hz hum excess and click density over
    > one timeline range at one point; the floor is the 10th-percentile short window, not a detected
    > silence, so continuous speech reads a higher floor and a lower SNR; evidence only."

    AU4 measured that moving a clause into the first sentence costs 4–14 B against a second sentence
    nothing reads, which is why the bias clause is in sentence one.
77. Arguments follow the audio family's flat range idiom (`get_audio_levels` server.rs:10524-10535):
    optional `start_frame` / `end_frame`, both omitted meaning the whole timeline, either alone
    filled from `TimeCode::ZERO` / `document.duration`, `start >= end` refused **by name**; plus at
    most one of `track` / `bus`, else master, exactly as `get_audio_spectrum` does (:10541-10556).
    The rendered text names every figure with its unit, prints `none` for an absent one through
    `render_optional_hundredths`, and repeats the percentile clause in prose so a reader of the text
    — not just of the schema — knows what the floor is.

### 4.2 `effect_documentation`'s profile hatch and `render_effects`' compact arm (R23)

78. **The hatch is mandatory, not economical.** `effect_documentation()` (schema.rs:575-694)
    enumerates every descriptor into five generated tools' descriptions — `add_effect`,
    `insert_effect`, `set_effect_param`, `set_effect_keyframes`, `clear_effect_keyframes` (spliced
    :437-446) — and its ordinary rows measure **≈ 39 B** each while a **profile** row
    (`profile_band01_tenth_db=-1200..=0, neutral -1200` plus its `", "`) measures **≈ 49 B**
    (R23, R46), so AU5's 45 rows would cost `31 × 49 + 14 × 39 ≈ 2 065 B` per spliced tool ×
    5 ≈ **10.3 kB** of `description_bytes` unhatched. It filters
    `is_noise_profile_parameter(name)` out of the row loop and appends
    `noise_profile_pattern_documentation()`, on the four hatches already in place (`color_curves`
    :584-590, `lut_node_pattern_documentation` :696-728, `matte_pattern_documentation` :730+,
    `audio_parametric_eq_pattern_documentation` :672-677):

    > `profile_band{01..31}_tenth_db=-1200..=0, neutral -1200`, one per ISO third-octave centre
    > 20 Hz..20 kHz low to high; write all 31 or none; learn them with `plan_dialogue_repair`.

    Its bounds are **read from the descriptor**, so the sentence cannot drift from §2.1's table. The
    14 remaining rows plus one pattern land at ≈ 4.0 kB, a saving of ≈ 6.3 kB. Every byte figure in
    this contract is an **estimate to be measured and re-derived** when the code lands (R23).
79. **`render_effects` gains one arm.** A repair node renders through `render_effects`
    (render.rs:1013-1044) unchanged — `id:name(param=v; keyframes=…)` — except that the 31 profile
    rows are collected out of the parameter loop and spelled once as `noise_profile=[-720,-735,…]`.
    R23 measures the enumerated form at **898 B** and the compact form at **170 B**. It needs **its
    own arm** rather than a `render_param` call, because `render_param` (:1046-1052) renders `Text`
    through `{value:?}`, i.e. quoted. The block is **omitted entirely** when every band is at the
    neutral or absent, so an unlearned node renders exactly as it does today.
80. Both golden pairs are extended on AU4's default-omission idiom (render.rs:1415, :1505):
    `au5_timeline_state_renders_a_learned_noise_profile` and
    `au5_timeline_state_omits_an_unlearned_noise_profile`. **Every existing golden stays
    byte-unchanged.** A room-tone fill clip needs no render surface at all — it is an ordinary media
    clip on the existing media-clip line (render.rs:185-198) — so §5.5 adds nothing to `render.rs`.

### 4.3 Part A registry effect and the ledger procedure (AU4 rule 87, R22)

81. **Part A adds one capability**, `get_audio_repair`, a `CapabilityKind::Inspector` by the `get_`
    prefix (runtime.rs:165-181). `INSPECTOR_TOOL_NAMES` (schema.rs:16) — the whole **hand-written
    capability** list, not an inspector list (R22) — goes `[&str; 80]` → `[&str; 81]`; the registry
    goes **134 → 135**; generated operations stay **54** of 57 `Operation` variants.
82. **The served quad `(7, 5_660, 3_510, 998)` is byte-identical in both parts.** No AU5 capability
    is in `COMPACT_TOOL_NAMES` (runtime.rs:17) and AU5 adds no `Operation` variant, so the seven
    served tools cannot move. That is the cheapest agent surface any audio slice has had and the
    ledger says so. A new **descriptor** costs zero input-schema bytes, because the `Operation`
    schema embeds `Effect.parameters` as an untyped map (server.rs:22986-22990); the cost is §4.2's
    description bytes plus `get_audio_repair`'s own schema and prose, ≈ 2–4 kB on `get_audio_qc`'s
    measured model (2 054 + 890 B).
83. **Procedure, exactly AU4 rule 87** (docs/AU4:1486-1497): change the surface → run
    `served_surface_is_small_and_keeps_the_internal_registry_discoverable` (server.rs:22961) and read
    the printed `ToolSurfaceMetrics` (:22972) → update the four asserts (serialized
    `(1_524_370, 5_660)` :23164-23171, `input_schema_bytes 1_389_434` :23172-23175,
    `description_bytes 112_948` :23176-23179, the served quad :23183-23192) → extend the derivation
    comment (:22981-23163) with a per-part byte split **that sums exactly** → update
    `cc7_the_agent_surface_is_unchanged_by_this_slice` (mcp_server.rs:2496), its running doc-comment
    ledger (:2464-2495), its counts (:2520-2556) and its ordering asserts (:2560-2598) → move
    `INSPECTOR_TOOL_NAMES`' length and server.rs:20695 → add the tool to the M36 1 KB
    description-budget list (server.rs:20705-20727) → append one M36 row plus its prose paragraph.
    The regenerated figures land as §0 errata.

### 4.4 `has_gain_computer` in core, and the reduction meter for free (F16, R14)

84. **`has_gain_computer` exists twice today** — `mixer_pane_ui.rs:1261-1266` driving `reduction_bar`
    (:1576-1596), and a byte-identical private copy at `audio.rs:222-228` driving
    `gain_reduction_keys`. AU5 **hoists it to core**, `pub fn has_gain_computer(effect: &str) -> bool`
    in `effect.rs` beside `is_audio_effect`, re-exported from `lib.rs`; both copies are deleted and
    both callers read the core one, which gains `"audio_denoise"`. Had only the app copy been
    changed, the bar would have had no data.
85. `take_gain_reduction` (audio.rs:912-928) gains a `Denoise` arm returning `minimum_gain`, exactly
    as its four existing arms do, publishing through `MixPeaks.gain_reduction`. **No new
    `MixerUnit`, no new telemetry field, no new `MixPeaks` field.** Hum and de-click are not gain
    computers and get no bar; the match returns `None` for them, as it already does for every other
    state.

### 4.5 The three `BaselineProofAnalysis` arms (R16)

86. **"No UI in Part A" means no new editing surface, not "no app code."** `BaselineProofAnalysis`
    (color_qc_ui.rs:254-260, `impl Analysis` :272-552) lives in the app, wraps `Arc<dyn Analysis>`,
    overrides exactly one method and forwards the other thirty-two. A new trait method with a
    `NotImplemented` default **compiles silently** if its arm is forgotten — only un-defaulted
    methods force an error, and `analysis_jobs` is already silently un-forwarded — so the three new
    methods get three one-line forwarding arms in Part A, each carrying the verbatim reason the file
    already gives three times (:455-458, :467-470, :479-482): "Without this arm the proxy would
    answer `NotImplemented` while the engine behind it can measure, which is the one way this wrapper
    can change an answer rather than only sharing a working proof." Each arm gets its own test on the
    `au3_the_baseline_proof_analysis_forwards_the_audio_measurements` template
    (color_qc_ui.rs:2738), using a double that answers **typed** errors so "forwarded" is
    distinguishable from "defaulted".
87. Every other `impl Analysis for` site — media engine.rs:758, app project.rs:701, agent
    server.rs:15124 / color_scopes.rs:2156 / export_queue.rs:1458 / eval.rs:11369, core media.rs:2503
    and the two core test doubles — takes the `NotImplemented` default and needs no arm, because none
    of them is a **proxy**. Agent `NoopMedia` (server.rs:15040-15084) gains three optional
    `Box<dyn Fn(..)>` doubles on the `mix_levels` / `audio_qc` idiom (:15082, :15072) so Part B's
    planner unit tests can drive them without a decoder.
88. **What Part A deliberately does not do.** No `Playback` change and so no `LiveAudioChange` work —
    the three methods are on `Analysis`, and repair edits are `UpsertAudioBus` / `SetAudioMaster`,
    already inside `is_audio_mix_operation` (app.rs:1679-1689). No new screenshot lane. No
    `DESIGN.md` change: Part A adds no surface a reader could look at.

---

# Part B — Room tone and repair planners

---

## 5. Part B core, media and agent

### 5.1 The room-tone store (D10, R26, R48)

89. **Normative.** Room tone is a **real WAV in the project asset store, registered as an ordinary
    `MediaAsset`**, on `LutAsset`'s digest-plus-store shape: bytes on disk and an ordinary
    `media_pool` entry, never a new document type. New
    `crates/kinewright-media/src/room_tone_store.rs` mirrors `lut_store.rs`: root
    `<project stem>.kinewright-assets/room-tone/<sha256>.wav`, reusing
    `LUT_STORE_SUFFIX = "kinewright-assets"` (lut_store.rs:33) with
    `ROOM_TONE_STORE_DIRECTORY = "room-tone"`, `sha256_bytes` for the digest, and the same
    `write_store_file` / `path_for` / availability vocabulary.
90. `ROOM_TONE_MAX_FILE_BYTES = 32 * 1024 * 1024` (R48), and — the rule lut_store.rs:36-41
    spells — **the length is checked from file metadata before a single byte is read**. 32 MiB
    accepts ≈ 87.4 s of 48 kHz stereo f32 (384 000 B/s) and matches `MAX_WAVEFORM_BYTES`
    (analysis.rs:28-32); `ROOM_TONE_MAX_CAPTURE_MILLISECONDS = 60_000` caps a legal capture at
    23.0 MB, comfortably inside it. **The two caps are deliberately not one number (R48):** the file
    cap guards the **reader** against a file nobody wrote — precisely what the LUT store's cap exists
    to catch (lut_store.rs:36-41, "keeping a mistaken pick … from being read into memory at all") —
    while the millisecond cap guards the **writer**, so the 9 MB between them is the margin and not
    an inconsistency to simplify away. `ROOM_TONE_MINIMUM_CAPTURE_MILLISECONDS = 500`; below it the capture refuses by name, which
    also clears R26's floor — `probe_path` rejects a duration rounding to **zero frames**
    (decode.rs:114-119), and an audio-only asset probes at `Rational::default()` = 30/1.
91. **Why the raw clip source and not the mix path.** The fill clip is replayed **through** the same
    track stage, bus and master chain the dialogue goes through. Capturing post-chain would apply
    the denoiser, the compressor and the limiter twice. The raw source is the only capture that
    makes the fill indistinguishable from the material either side of the cut.
92. *Rejected: a `ClipContent::RoomTone` variant* — **265** `ClipContent::` sites (R24) and three
    choke points where a new variant is **silent** (timeline.rs:243, derived.rs:963, qa.rs:213), plus
    a synthetic decode path in `AudioMixer::open` / `mix_pass` that does not exist, and it would be
    rejected by `validate_clip_audio` the moment it carried a gain. *Rejected: a `LutAsset`-shaped
    digest record* — the mix path can only decode things in `media_pool`; the **store** shape is the
    precedent, the **record** shape is not.

### 5.2 `Document::track_gaps` and the `qa_document` refactor (D11, R13)

93. **Normative, narrowly.**

    ```rust
    /// AU5 §5.2: the leading and interior gaps of one track, in project frames.
    ///
    /// Content- and kind-agnostic: a title or a freeze closes a gap exactly as a
    /// media clip does, because this is the definition `qa_document`'s
    /// `"track_gap"` warning has always used. **Never the trailing gap** — a
    /// track that ends before the project does is not a hole, and reporting one
    /// would invent warnings on every existing document. Callers that want only
    /// audible gaps filter themselves.
    ///
    /// `None` when no track carries `track`.
    #[must_use]
    pub fn track_gaps(&self, track: TrackId) -> Option<Vec<Range<TimeCode>>>;
    ```

    **Addressed by `TrackId`, not by index (R42).** Every caller holds one: `plan_room_tone_fill`'s
    argument is `track` (rule 102), the QA walk carries `track.id` (qa.rs:135-137), and
    `Document::track_audible(&self, track: TrackId)` (model.rs:1220) is the house shape. The name
    `Document::track` is already taken by the audio-mix accessor (model.rs:821), which is why the
    accessor is `track_gaps` rather than `track(..).gaps()` — a naming problem, never an addressing
    decision. An unknown `TrackId` answers **`None`**, distinguishable from a track with no gaps.

    Seeded at `TimeCode::ZERO`, advancing `previous_end = end.max(previous_end)` — verbatim
    `qa_document`'s walk (qa.rs:136, :225-236, :277-279, :342).
94. `qa_document` is refactored onto it so "what is a gap" has **one** definition, and the QA output
    is pinned **identical** on the existing corpus:
    `qa_reports_missing_media_gaps_retiming_and_caption_readability` (qa.rs:684, code asserted :745)
    stays green unchanged, and a new test asserts the whole `QaReport` is equal before and after the
    refactor on a document with a leading gap, an interior gap, a trailing gap, adjacent clips, a
    title closing a gap, an empty track and a track whose only clip starts at 0.
    `plan_room_tone_fill` filters to `TrackKind::Audio` and audio-bearing neighbours itself.

### 5.3 The fill arithmetic (D11, R12)

95. **Normative.** A fill is one or more `AddClip`s of the room-tone asset on **the same track**,
    starting at `gap.start`, butt-joined, **with no fade**, tiling forward, capped at
    `ROOM_TONE_MAX_TILES = 64`.
96. **Why no crossfade, and why the same track.** Clips on one track cannot overlap (`ClipOverlap`),
    so there is nothing to fade across, and two ramps meeting at an abutment are a **dip to
    silence** — the exact artefact the fill removes; a fade needs the fill on its own track,
    overlapping, where the ramp *adds* to material that already carries its own floor and the join
    **swells** by up to 6 dB instead of stepping. A butt join of broadband room tone to broadband
    room tone is inaudible, which is why the craft works this way. And a track routes to at most one
    bus, so a dedicated room-tone track would have to join the dialogue bus's `tracks` list — a
    second operation, a second thing to keep in sync, and a fill *not* processed by the chain the
    dialogue is — while a same-track fill makes the `"track_gap"` warning disappear, the only
    observable "the gap is gone" signal in the product. Friction accepted: a later trim into a filled
    gap fails with `ClipOverlap`, exactly as trimming into any neighbour already does.
97. **Normative, the exact-gap rule (R12).** `time.rs` gains the inverse of
    `map_source_range_to_project` (:229-243):

    ```rust
    /// AU5 §5.3: the source end whose mapped project span is exactly
    /// `project_duration`, searched in the two-frame window around the rounded
    /// estimate. `map_frames` rounds to nearest (time.rs:200-207), so not every
    /// project duration is representable: a 60 fps project reading a 30 fps
    /// audio-only asset can only express even-frame spans. The caller is told,
    /// never silently given the wrong length.
    pub fn map_project_duration_to_source(
        source_start: TimeCode, project_duration: TimeCode,
        source_fps: Rational, project_fps: Rational,
    ) -> Result<TimeCode, TimeMappingError>;
    ```

    The planner and the button then, per gap: compute the candidate end, **shrink by one asset frame
    on overflow, never overlap**, assert `document.clip_duration(&fill) == gap.end − gap.start`,
    and — when no end in the window maps exactly — **skip the gap** with the reason `"gap of {n}
    project frames has no exact source range at {source_fps}/{project_fps}"`, on `plan_clip_fades`'
    "never fail a whole plan for one clip" idiom (server.rs:8734-8790); **and the fill clip is
    written at `speed_percent = 100`** (R44), so `clip_effective_fps(asset.fps, fill) == asset.fps`
    (model.rs:1250) and rule 97's helper really is the inverse of `clip_duration`, which maps through
    `clip_effective_fps` and therefore folds in `speed_percent`. A retimed fill is an ordinary edit
    afterwards; the planner never proposes one.
98. **Why this is not a 30 fps detail.** An audio-only asset is probed with
    `fps = Rational::default()` = **30/1** (decode.rs:105-112), the mix maps `source_range` through
    `asset.fps` (audio.rs:2977-2983) and `clip_duration` through `map_source_range_to_project`
    (model.rs:1250-1251). At a 24 or 25 fps project a gap of `G` project frames needs
    `G × (30 / project_fps)` asset frames, rarely an integer, so without rule 97 the fill lands one
    frame short — a residual gap that keeps the `"track_gap"` warning — or one long, which is
    `ClipOverlap` and a refused plan. §5.4's 30 fps fixture hides the entire class, which is why it
    gains two more arms.

### 5.4 The seam fixture and its budget (exit-gate clause 2, R12)

99. Room tone = `wav_f32(pseudo_random_amplitude(96_000 × 2, 0.010))` — the argument is a **sample**
    count, so this is 96 000 frames × 2 channels = 2 s of 48 kHz stereo (R45) — registered as an
    asset. Timeline at 30 fps: clip A frames `0..30`, **gap `30..60`**, clip B
    frames `60..90`, all from a dialogue fixture. Fill = one `AddClip` of the room-tone asset at 30
    over source `0..30`. Reference = `decode_audio_range(room_tone_path, 30 fps, 0, 30, 48_000, 2)`.
100. **The pin.** Over `[gap.start − 10 ms, gap.end + 10 ms]`, `mix_audio`'s output equals the
     concatenation of [clip A's own decode | the reference | clip B's own decode] to within
     `ROOM_TONE_SEAM_BUDGET = 1.0e-4` per sample. **Expected: exactly 0.0.** Each `AudioMixSource`
     opens its **own** `AudioDecoder` at its own `source_sample_start` (audio.rs:2977-3005,
     :3486-3535) and the resampler is created per decoder from the first frame (:3587-3601); with a
     **48 kHz PCM** fixture swresample does format conversion only — no rate filter, no state to
     prime — so the difference is **exact, not 1e-7**. The fixture stays at 48 kHz and the doc comment
     says why; the claim does **not** generalise to a rate-converted source, and that sentence is
     part of the pin. A zero deviation prints `margin=unbounded`, AU3's idiom. This one assertion
     catches an off-by-one fill start, a stray fade, a level error, a resample-phase error and a
     wrong source range — every plausible way a fill stops being seamless. Print
     `AU5_ROOM_TONE_SEAM measured=… budget=1e-4 margin=…`.
101. **Three more arms (R12).** (i) A 90-frame gap tiled from the 60-frame sample, asserting **all
     three** joins to the same budget — clip A → tile 1, tile 1 → tile 2, tile 2 → clip B, of which
     one is internal to the fill (R46 corrects the draft's "both internal joins": two tiles produce
     three joins). (ii) A **25 fps** project reading the 30 fps audio-only asset
     over a **7**-frame gap: `map_frames(8, 30, 25) = round(40/6) = 7`, so source `0..8` maps to
     exactly 7 project frames, `clip_duration(fill) == 7`, and the seam holds. (iii) A **60 fps**
     project over a **7**-frame gap: `round(2E) = 7` has no solution, so the gap is **skipped** with
     rule 97's reason while the plan still commits its other gaps.

### 5.5 `plan_room_tone_fill`

102. Arguments (`RoomToneFillPlanArgs`, `deny_unknown_fields`, optionals `#[serde(default)]
     Option<T>` on `AudioDuckingPlanArgs`' shape, server.rs:10156-10190): `track`, optional `range`,
     optional `asset_id` defaulting to the sole registered room-tone asset, `minimum_gap_frames`,
     `maximum_tiles`. Defaults are **named consts, not serde** (server.rs:8892-8925):
     `ROOM_TONE_MINIMUM_GAP_FRAMES = TimeCode(1)`, `ROOM_TONE_MAX_TILES = 64`.
103. It enumerates `Document::track_gaps`, filters to the requested `range` and to gaps at or above
     the minimum, applies rule 97 per gap, refuses beyond `maximum_tiles`, and reports per gap
     `{ start, end, tiles, skipped_reason }`. It **never fails a whole plan for one gap**, and when
     nothing is proposed it returns `success_structured` with `prepared_edit_plan: null` plus the
     reason that applied. It raises **no confirmation** — `plan_confirmation_description`
     (server.rs:14914-14952) fires only on `DeleteClip`, `RippleDeleteClip` and a non-empty
     `RemoveTrack`, and a fill removes nothing — and it ends at `prepare_operations`
     (server.rs:7739) like every other planner, mutating nothing.
104. **Idempotent by construction:** a filled gap is not a gap, so a re-run proposes nothing. And a
     fill clip is **not marked** — it is an ordinary media clip of a room-tone asset, so every AU4
     survival rule applies unchanged (`survive_clip_edit` on trim/split/roll/slide/speed, ride-along
     on ripple, removal on delete), it carries no envelope so `E` is a no-op on it, and deleting it
     re-opens the gap and brings the QA warning back. *Rejected: a `Clip.is_room_tone` flag* — 146
     literal edits, a survival rule, a render suffix and a validation arm for a boolean nothing reads.

### 5.6 `plan_dialogue_repair` (D13, R10, R30)

105. Arguments: `tracks`, optional `range`, `denoise` / `hum` / `declick` flags (all defaulting true),
     `reduction_tenth_db` (**default 120**), `hum_fundamental_hertz` (default: whichever of
     `get_audio_repair`'s two measured excesses is larger), `minimum_snr_gain_db_hundredths` (default
     **100** = 1.0 dB), `replace: bool`. All defaults are named consts.
106. **The measured loop, which is the deliverable.** Measure `Analysis::audio_repair` on the
     candidate point **before**; learn the profile with `mix_noise_profile` over the longest silence
     span; build the chain; apply it to a candidate document; measure **after**; and **refuse** —
     `Ok(error_text(…))`, tool text, never a protocol error (server.rs:12117) — when
     `snr_gain < minimum_snr_gain_db_hundredths`, naming **both** numbers and the direction of the
     percentile bias (rule 21). That sentence is the row's "where the model can measure improvement",
     made a gate rather than a claim. Unlike AU4's `measured: null` idiom, an unmeasurable
     improvement **refuses**, because refusing is the deliverable.
107. **The readiness gate (R10).** The learn range comes from `Analysis::timeline_silences`
     (media.rs:1992-1997), which is fed by a per-asset **async** analysis and returns `Ok(vec![])` on
     an unanalysed project. The planner gates on `silence_status` and calls
     `request_silence_detection` exactly as `plan_audio_ducking` does (server.rs:8559-8566, direct
     call :8581) and refuses with that planner's own wording, `"silence analysis is not ready"`, as a
     **distinct** arm from `"no silence span reaches 469 ms"`. The AU4 real-engine test already
     tolerates exactly one such refusal (tests/mcp_server.rs:7433-7457) and AU5's does the same.
108. **The chain it builds**, in signal order, at the **head** of the bus: `audio_denoise` (profile
     written, `reduction_tenth_db` from the argument, `lookahead_milliseconds` 12) →
     `audio_hum_removal` (`fundamental_hertz`, `harmonic_count` 3, `depth_tenth_db −300`,
     `notch_q_hundredths` 1200) → `audio_declick` (`max_click_milliseconds` 1,
     `detector_threshold_tenth_db` 240, `lookahead_milliseconds` 3). Declared 15 ms (§2.3). Effects
     are written through `static_audio_effect` (server.rs:9630) — `parameters` only, never
     `keyframes`.
109. It **reuses the track's bus** if it has one, preserving its existing effects list and inserting
     the repair prefix at the head; otherwise it creates one, allocating through
     `AudioMix::next_bus_id()` and the shared `max + 1` effect-id scan (server.rs:9535-9550). A track
     routes to at most one bus, so there is no other choice. It refuses when the resulting chain
     would exceed `CHAIN_LOOKAHEAD_MILLISECONDS`, quoting the existing chain's declared milliseconds
     and its own 15, rather than letting `UpsertAudioBus` fail inside a prepared plan. **No source
     separation model** (§1.3).

### 5.7 The `normalization_context` amendment (D14, R11)

110. **The problem, stated exactly.** `validate_audio_mix` raises `TrackInMultipleAudioBuses`
     (operation.rs:4611-4619), and `normalization_context` (server.rs:9511-9521) **refuses outright**
     when the requested tracks intersect any existing bus. So without an amendment a repaired
     dialogue track can never be normalized, and AU6's noisy-location-dialogue workflow has no path.
111. **Normative (R11).** `normalization_context` gains one relaxation and returns a richer answer:
     when the intersecting bus's **every** effect is an AU5 repair node **and** the bus's `tracks`
     equals the requested set exactly, it returns **extend bus N** rather than *new bus* — carrying
     the existing `id`, `name`, `tracks`, **`gain_tenth_db`**, `gain_curve` and
     `ducking_sidechain_tracks`, i.e. **every `AudioBus` field except `effects`** (model.rs:497-517),
     which is the repair prefix rule 112 appends to — instead of refusing. Enumerating the fields is
     deliberate (R35): `normalization_bus` (server.rs:9726-9734) builds a whole fresh `AudioBus`
     today and writes `gain_tenth_db: 0`, so a field left off this list is a field silently reset —
     and because `gain_tenth_db` is `skip_serializing_if = "i32_is_zero"`, the reset would be
     invisible in the golden too. B11 gains an arm with a non-zero bus fader and a `gain_curve`,
     asserting both survive both orders. Any other intersection still refuses with the existing
     message. The `tracks`-equality condition is not optional: without it, normalization would
     silently re-target a different set.
112. `normalization_bus` (server.rs:9642-9734) takes the existing prefix as an argument and
     **appends** to it rather than starting from an empty `Vec` (:9655), keeps the existing bus's
     name rather than writing `"Delivery normalization"` (:9727), and still ends in
     `audio_true_peak_limiter` at 5 ms. The prefix rides **all four** convergence iterations of
     `verified_normalization_operation` (:9573-9596), so the loudness converged on is the
     **repaired** loudness — desirable, and the convergence is re-verified by rule 113's test rather
     than assumed. Total declared: 15 + 5 = **20**, exactly the budget.
113. **The pin.** One test runs `plan_dialogue_repair` and `plan_audio_normalization` **in either
     order** on the same document and asserts the same end state: one bus, repair prefix then
     compressor/gain then true-peak limiter, `chain_lookahead_milliseconds == 20`, and the measured
     integrated loudness inside AU3's tolerance. The in-crate refusal pin
     `normalization_context_refuses_each_malformed_argument` (server.rs:26075, assertion
     :26140-26144) is **updated** for the relaxed arm and gains a negative case — a bus carrying one
     `audio_gain` still refuses. **Nothing under `crates/kinewright-agent/tests/` pins the refusal
     string**, so AU3's g2/g3 eval baselines carry no repair bus and are untouched; the relaxation
     only widens acceptance.

### 5.8 `capture_room_tone` (D10, R22, R26)

114. A hand-written capability on `import_lut_asset`'s exact shape (lut_store.rs:352-369), inferred
     as **`CapabilityKind::Action`** by runtime.rs:165-181 — no `get_`/`plan_` prefix, no
     `CAPABILITY_KIND_OVERRIDES` entry (R22). Arguments: `asset_id`, `source_start_frame`,
     `source_end_frame`, optional `name`.
115. It decodes the chosen **source** range with
     `decode_audio_range(asset.path, asset.fps, from, to, 48_000, 2)` (audio.rs:41-70), writes
     `wav_f32` bytes into §5.1's store under their own sha256, probes the written file with
     `probe_path` like any import, and submits **one** `Operation::AddAsset { asset }` — an ordinary
     generated operation (schema.rs:270), so **no new `Operation` variant**. The asset carries a
     `name` (R26), defaulting to `"Room tone — {source asset name}"`. It sits behind the same
     confirmation path as `import_lut_asset`, because it is the only AU5 tool that **writes bytes
     into the project directory**. Note that `probe_path` already writes a `name` from
     `path.file_name()` (decode.rs:122-125), which for a store file is `<sha256>.wav`:
     `capture_room_tone` therefore **overrides** probe's answer rather than carrying it (R46).
116. **Idempotence:** when the store already holds that digest and `media_pool` already carries an
     asset at that path, it returns the existing `asset_id` and emits **no operation**, so a re-run
     is a no-op rather than a `DuplicateAsset` failure. The app reaches the same store directly,
     exactly as it already reaches `LutStore::import_lut_asset` (media_workflow.rs:1587, :1877),
     which is what makes §6.4's button a real editor path with no agent in the loop.

### 5.9 Part B registry effect and the ledger (R22)

117. Part B adds **three** capabilities — `capture_room_tone` (Action), `plan_room_tone_fill`
     (Planner), `plan_dialogue_repair` (Planner). `INSPECTOR_TOOL_NAMES` goes **81 → 84**; the
     registry goes **135 → 138**; generated operations stay **54**; the served quad
     `(7, 5_660, 3_510, 998)` is **byte-identical**. Measured models for the estimate:
     `plan_audio_ducking` 2 391 + 1 000 B, `plan_clip_fades` 755 + 845 B — order +2 to +4 kB each of
     `input_schema_bytes` plus prose. All three descriptions go under M36's 1 024 B exclusive budget
     with their load-bearing clause in the **first sentence** and join the M36 budget list
     (server.rs:20705-20727); three M36 rows, one per capability, each with its measured byte delta.
     Procedure exactly as rule 83; the regenerated figures land as §0 errata.

---

## 6. Part B app surfaces

### 6.1 The three Mixer cards (D2, R15, R17, R24, R26)

118. `INSERTABLE_AUDIO_EFFECTS` (mixer_pane_ui.rs:1605-1612) goes `[&str; 6]` → `[&str; 9]`, with
     `audio_denoise`, `audio_hum_removal` and `audio_declick` appended after `audio_gate`. Legacy
     `audio_eq` / `audio_limiter` remain unoffered. **Both doc comments that say "six" move with the
     array** (R46): mixer_pane_ui.rs:1602 ("The six nodes the mixer's `+ Effect` menu offers") and
     :1616 ("Whether the mixer's `+ Effect` menu offers one audio effect (AU2 §6.8)"), whose sibling
     assert message at :1699 ("the six nodes of AU2 §6.8") rule 119 replaces anyway. Both are named
     in §8's app line.
119. **The tautological `debug_assert` is replaced, not re-worded (R26).** mixer_pane_ui.rs:1698-1701
     re-asserts membership of the array it iterates. It becomes: every name in
     `INSERTABLE_AUDIO_EFFECTS` satisfies `kinewright_core::is_audio_effect`, has an
     `effect_descriptor`, and every one of that descriptor's parameters resolves to a `MixerUnit`
     other than `Plain` **or is named in a short allow-list** — a claim that can fail.
120. `effect_display_name` gains three arms (`Denoise`, `Hum removal`, `De-click`) and
     `parameter_label` gains nine, on its existing 25-arm table (:1414-1452). The 31 profile rows
     need none: `card_body` skips them by `is_noise_profile_parameter` exactly as it skips `bypass`
     by exact name (:797-813) — they are read from the well, never dragged. `insert_audio_effect`
     skips them too (rule 6).
121. **`mixer_unit` needs one amendment, not a new variant.** Every AU5 suffix is already mapped —
     `_tenth_db` → Decibels, `_hertz` → Hertz, `_milliseconds` → Milliseconds, `_q_hundredths` → Q
     (exactly why R17 renames the hum Q to `notch_q_hundredths`; a bare `q_hundredths` would fall to
     `_hundredths` → Ratio and read "12.0:1"). **`harmonic_count` is the first audio parameter to
     reach `MixerUnit::Plain`**, so the doc comment at :1289-1291 — "No audio descriptor has one
     today" — is amended in place to name it.
122. **No `+ Effect` change and no new reduction surface.** `insertion_block` (:1636-1649 — R46
     corrects the draft's :1622-1635) already greys the menu entry with `LOOKAHEAD_BUDGET_SPENT`
     because `insertion_lookahead_milliseconds`
     (:1628-1633) reads the descriptor neutral, so `audio_denoise`'s `12..=12` row counts correctly
     with no app edit; and the denoise card's readout is the existing `reduction_bar`, because §4.4
     gave `has_gain_computer` its name and `take_gain_reduction` its arm.

### 6.2 The noise well and the hum comb (R15)

123. **`eq_well` is not reusable "on its exact scaffolding" (R15).** mixer_pane_ui.rs:1528-1574 is
     private, takes `&Effect`, and its magnitudes are hard-wired to `parametric_eq_magnitude_db`
     through `eq_well_magnitudes` (:1484-1494), with `EQ_WELL_SAMPLES = 96`, a log 20 Hz–20 kHz x-map
     (:1478, :1499) and a ±24 dB y-map (:1508) baked in.
124. **The hum comb** parameterises exactly one thing: `eq_well_magnitudes` takes the magnitude source
     as an argument, so the comb is `eq_well` whole, drawn from `hum_removal_magnitude_db` (rule 51)
     at `MIXER_EQ_CURVE_HEIGHT = 96.0` — same x-map, y-map, 1.6 px polyline, `eq_well:{id}` rect and
     "Magnitude at 48 kHz." tooltip.
125. **The noise well is a new function**, sharing only the well chrome — `LETTERBOX` fill,
     `theme::paint_inset_well`, the `BORDER_SUBTLE` stroke. `MIXER_NOISE_WELL_HEIGHT = 48.0`, 31 bars
     against a **−120 … 0 dB** scale in `TEXT_SECONDARY`, low to high, with the `TEXT_MUTED` sentence
     **"No profile learned."** when every band is at the neutral. Rect `noise_well:{id}`. It is
     **never** the accent: DESIGN.md:59-69 reserves status colours for outcomes and :418-421 already
     says "quiet is never a failure", which is the sentence the well is drawn to. Both wells are
     **pure geometry plus formatting** and are tested as such, without a window.

### 6.3 The `Learn` gesture (R10, R24)

126. **The smallest surface that can reach a document.** A `Learn profile` button in the denoise card,
     returned up the channel `LOUDNESS`'s `Reset` already uses:
     `MixerFrame { selection, reset_loudness, learn_noise_profile: Option<(AudioChain, EffectId)> }`
     (mixer_ui.rs:471-478). The pair is `Copy`, so `MixerFrame`'s existing `#[derive(… Copy …)]`
     (mixer_ui.rs:471) is unchanged, and `AudioChain` is the spelling `MixerSelection::chain()`
     already produces (mixer_ui.rs:286-290) and the reduction map is already keyed by (:177, :252) —
     AU5 adds no second name for "which chain" (R43), and there is no `MixerChainKind` anywhere in
     `crates/kinewright-app`. R24 is what makes this the right shape: document edits leave the Mixer
     through `edits: &mut InspectorEdits` (mixer_ui.rs:503, :519-528, :574) and `MixerFrame` carries
     only session and telemetry — so `learn_noise_profile` is a **request**, not an edit, and "the
     card pushes no operation" stays enforced by `InspectorEdits` rather than by `MixerFrame`.
127. `app.rs` owns the document and the `Analysis`. The **range** is the longest span from
     `Analysis::timeline_silences` **filtered by the caller** on `TimelineSilenceSpan.track`
     (media.rs:1489) to the tracks feeding that chain — the routine itself has no track argument
     (R46) — so no range-selection
     primitive (there is none anywhere in the app) and no new selection state is needed. The
     measurement runs on the colour-QC single-flight pattern in miniature, because
     `mix_noise_profile` decodes and must not block a frame: one `ActiveWorker`
     (color_qc_ui.rs:608-623), a monotonic generation (:682), `poll()` in `poll_background`
     (app.rs:981+), a **50 ms** repaint while pending (app.rs:1000-1002), and the newest request
     parked in `queued` rather than a second thread. On completion the app pushes **one** chain set —
     one undo entry under the existing `audio_bus:{id}` / `audio_master` key (rule 19).
128. **Two refusal arms, not one (R10).** Hover names the span that will be used. A `TEXT_MUTED`
     sentence refuses with **"Silence analysis is still running for these tracks."** when
     `silence_status` is not ready — and requests it — and with **"No silence span reaches 469 ms."**
     when it is ready and no span does. Collapsing them would tell the editor the recording has no
     silence when it means the analysis has not run. *Rejected: an inspector button on the clip's
     leading silence* — a clip's head is usually not silent, the inspector has no height discipline
     (`INSPECTOR_MAX_HEIGHT` is enforced by clipping alone), and the node being taught is on the
     **chain**, not the clip.

### 6.4 The timeline `Room tone` button (R25, R29)

129. One toolbar button, `Room tone`, beside `Envelopes` (timeline_ui.rs:808-820), 72 × 22 px, enabled
     only when a clip is selected and its track has a gap. It fills the gap **nearest the playhead on
     that track**, capturing from that track's longest silence first if no room-tone asset exists —
     one `DoBatch` of `[AddAsset?, AddClip…]`, one undo entry, errors through
     `InspectorEdits::errors`.
130. **The toolbar risk is real and measured (R25):** ≈ **687 px** with ripple off, ≈ **739 px** with
     the `RIPPLE` label on, in a plain `Layout::left_to_right` (timeline_ui.rs:766-770) with **no wrap
     and no scroll** — overflow clips silently, and `Fit`'s sizing (:847) already saturates at its
     240 px floor. **This button is the first thing cut if Part B runs hot** (R29): the fill
     deliverable is still discharged by `plan_room_tone_fill` plus `capture_room_tone`, exactly as AU3
     discharged normalization, and rule 116 keeps the store reachable from the app either way.
     *Rejected: a gap hit target or context menu* — there is no gap rect, no gap hit region and no
     context menu anywhere in `timeline_ui.rs`; building one is a deferral (§1.3), not a prerequisite.

### 6.5 Heights and DESIGN.md (R15, R25, R47)

131. **Four new ±1 px pins**, all in `mixer_pane_ui.rs`'s measured-budget style (`const BUDGET` beside
     `const MEASURED`, the figure printed, the doc comment recording where the budget came from). They
     are **new tests, not moved ones** (R25): there is no bus-pane pin today, and the nearest existing
     figures are the strip pins (mixer_ui.rs:2795-2799, BUDGET 240 / MASTER 210 / BUS 215).

     | pin | components | BUDGET | MEASURED |
     | --- | --- | --- | --- |
     | denoise card | group 8 + header 18 + 2 control rows ≈ 44 + noise well 48 + reduction bar 6 + spacing | **160.0** | §0 erratum |
     | hum card | group 8 + header 18 + 2 control rows ≈ 44 + comb 96 | **190.0** | §0 erratum |
     | de-click card | group 8 + header 18 + 1 control row ≈ 22 | **70.0** | §0 erratum |
     | bus pane, **hum card** expanded | title + `AUTOMATION` 120 + routing + sidechain + separator + cards + `+ Effect` | **520.0** (R47) | §0 erratum |

     **The bus-pane budget is a content height, not a fit budget (R47).** `chain_pane` opens with a
     `ScrollArea`, and `the_chain_pane_with_an_automation_section_still_fits_the_dock`
     (mixer_ui.rs:4786-4836) already asserts on today's master pane that the content **exceeds** its
     260 px viewport and that the scroll is "load-bearing, not decorative". A 520 px bus pane
     therefore clips nothing; it costs scroll distance, which makes 520 a content-height discipline
     number of exactly the kind the master pane's 420 / 398 pin (mixer_pane_ui.rs:2223-2234) already
     is, and makes the cut-order argument one about how far an editor scrolls to reach `+ Effect`.
     The measurement is taken with the **hum card** expanded — the tallest of the three at 190, so a
     figure measured with the de-click card (70) expanded would be 120 px optimistic (R46) — and if
     it overruns, `MIXER_NOISE_WELL_HEIGHT` 48 → 32 is the first cut, buying **16 px of scroll**
     rather than rescuing a hidden control. `MEASURED` lands as a §0 erratum, exactly as AU3's
     "the budget moves with the measurement recorded beside it" rule requires.
132. `card_body` lays four controls into `control_cell_width() = (400 − space::EIGHT)/3 = 122.67`
     cells inside `horizontal_wrapped`, so denoise and hum take **two** rows and de-click **one**;
     only one card is expanded at a time (`expanded_card` :679-689), which is what keeps the pane
     budget reachable. **The master pane is untouched** — none of the three sections it carries
     changes — so the 398 / 420 pin (:2223-2234) stands unchanged, and
     `the_chain_pane_with_an_automation_section_still_fits_the_dock` (mixer_ui.rs:4784-4837) is
     extended with a repair-bearing bus rather than replaced.
133. **DESIGN.md**, both sections, each pinned by the `include_str!` phrase tests that already slice
     them (mixer_ui.rs:4155-4200+, timeline_ui.rs:3914-3942):
     - `### Mixer` gains one paragraph: the three repair cards and where they sit in the chain; the
       noise well as "the floor the node was taught, not a meter — quiet is never a failure"; the
       `Learn profile` button and that it reads the longest silence on the tracks feeding the chain;
       and "The three repair nodes declare fifteen of the chain's twenty milliseconds, which is why a
       repaired bus can still take a delivery limiter and nothing else."
     - `### Timeline` gains one paragraph: the `Room tone` button, the nearest-gap-on-that-track rule,
       and that a filled gap is an **ordinary clip** — it trims, splits and deletes like any other,
       and deleting it brings the gap warning back.

---

## 7. Acceptance checklists

### Part A (A1–A18)

Closes exit-gate clause 1 verbatim from ROADMAP-AND-WORKFLOWS.md:649 — *"Repair is measured on
synthetic corruptions with pinned SNR gains"* (A4, A7–A11) — and the node, profile and measurement
half of the row's deliverables (§1.2).

A1. The three descriptors carry **exactly** §2.1's rows, ranges and neutrals: `EFFECT_DESCRIPTORS` is
    28 long, `is_audio_effect` accepts eleven names, the 31 profile rows are `-1200..=0` neutral
    **-1200**, `audio_denoise`'s `lookahead_milliseconds` is `12..=12` and `audio_declick`'s `3..=3`,
    `audio_hum_removal` has **no** `lookahead_milliseconds` row, the Q row is `notch_q_hundredths`,
    and `max_click_milliseconds` is `0..=1` neutral 0; twelve new `EffectUniform` variants exist, all
    31 profile rows share one, all three `bypass` rows reuse `AudioBypass`, and every variant is in
    the compositor's ignore arm; a pre-AU5 document round-trips byte-identically and an all-neutral
    repair node serialises with **no** profile rows — core `tests/au5_core.rs`, `tests/contracts.rs`.
A2. **B1**: for **every** `is_audio_effect` descriptor, media's
    `node_lookahead_milliseconds(&bare_effect)` equals core's
    `chain_lookahead_milliseconds(&[bare_effect])`, with `audio_hum_removal` (no row → 0),
    `audio_true_peak_limiter` (5) and `audio_denoise` (12) named as explicit cases; and an
    `audio_denoise` whose `lookahead_milliseconds` is **absent from the parameters map** builds a
    512-frame window and passes A5's impulse test — media `audio.rs`.
A3. `is_static_audio_parameter` is true for exactly the three pre-AU5 pairs plus
    `("audio_denoise"|"audio_declick", "lookahead_milliseconds")` and every
    `is_noise_profile_parameter` row of `audio_denoise`, and **false** for `max_click_milliseconds`;
    `is_hold_only_parameter`'s audio branch reads `bypass | detector | true_peak | harmonic_count`; a
    keyframe on a lookahead row is rejected with `"sets processing latency and cannot be keyframed"`
    and one on a profile row with **`"is read when the chain is built or retuned and cannot be
    keyframed"`**, with `au2_core.rs:634` and `docs/AU2-EQ-AND-DYNAMICS.md:99` updated and green; a
    non-`Hold` `harmonic_count` key raises `NonHoldKeyframeParameter`; `is_noise_profile_parameter` is
    `pub`, re-exported, and true for exactly 31 names — core `tests/au5_core.rs`, `tests/au2_core.rs`.
A4. **B3, R33, R34**: §3.11(a′) learns the 1 kHz band within
    `DENOISE_PROFILE_LEARN_BUDGET_TENTH_DB` of its analytic level and gates it within
    `DENOISE_PROFILE_GATE_BUDGET_TENTH_DB` of `L − 40 dB` with every neighbouring band moving under
    3.0 dB; rule 44's conversion is a named function tested directly at three window sizes, including
    the `max(1, bins_in_band)` floor at 20 Hz and ≈ 49 bins at 20 kHz; **`third_octave_spectrum`'s
    parameterisation leaves AU2's `mix_spectrum` and `measure_mix_spectrum` goldens byte-unchanged**
    when its callers pass `SPECTRUM_SEGMENT_FRAMES / SPECTRUM_HOP_FRAMES / SPECTRUM_MINIMUM_FRAMES`,
    and a 22 528-frame range is accepted at 4 096 / 2 048 / 22 528 where the unparameterised routine
    answered `None`; the eleven `window_limited` bands (20 Hz … 200 Hz) are reported as **learned**;
    rule 43's **three conversions** are each driven directly — a learn over digital silence writes
    31 × −1200, a rounded −724 hundredths becomes **−73** tenths by `div_euclid(10)`, and a band
    louder than a full-scale sine **clamps to 0** rather than failing `SetEffectParam` inside a
    prepared plan; both margins are printed — media `tests/au5_fixtures.rs`.
A5. **S1 and R19**: `inverse_fft(forward_fft(x))` returns `x` within 1e-12 at 512 / 4 096 / 16 384;
    the twiddle-table `forward_fft` is **`to_bits()`-identical** to the pre-AU5 routine at those
    lengths and AU2's spectrum goldens are byte-unchanged; each node's **declared** delay is pinned
    by an impulse through `process_buffer_static` landing at frame 0, at 44.1 / 48 / 96 kHz, bypassed
    and not. Because rule 41 puts an all-neutral `audio_denoise` on the `direct` branch, that impulse
    exercises the delay line only; the **OLA delay is pinned separately and directly** (R32): a unit
    test constructs `DenoiseState` at each rate, forces `g_k ≡ 1.0` for every bin and every block,
    pushes `4 × window` frames of `pseudo_random_amplitude`, and asserts the output equals the input
    delayed by exactly `window − 1` frames to within **1e-12** over the fully-overlapped region
    `[window − hop, …)`. Hann²/1.5 reconstruction is exact only up to rounding, which is why this arm
    carries a tolerance and the `direct` branch exists (R49). A second arm asserts
    `output_pad.len() == latency − (window − 1)` equals **65 / 18 / 129** at 48 / 44.1 / 96 kHz — the
    one place the arithmetic is restated, because the impulse cannot reach it. Rule 41's `direct`
    toggle mid-buffer never dips; `AU5_PREROLL` prints with no timing assert — media `audio.rs`,
    `tests/au5_fixtures.rs`.
A6. **B2 and S3**: the construction assert holds over the whole `max_click_milliseconds` domain at
    44 100 / 48 000 / 96 000 Hz with §3.5's six figures asserted individually and is a real `assert!`
    reachable in release; the trailing reference **excludes flagged samples**, proved by a direct test
    that the same detector with a naive window finds **zero** clicks on §3.11(c)'s fixture and the
    specified one finds nineteen — an arm that has something to find only because §3.11(c)'s click is
    **sign-alternating per frame** (R31), so every one of its eight frames carries the full `4 × 0.9`
    second difference — media `audio.rs`.
A7. §3.11(a): the noise-floor drop is ≥ `DENOISE_FLOOR_DROP_BUDGET_TENTH_DB` and the 1 kHz loss ≤
    `DENOISE_TONE_LOSS_BUDGET_TENTH_DB`, both with ≥ 2× margin, printed as `AU5_DENOISE` — media
    `tests/au5_fixtures.rs`.
A8. §3.11(b): the hum drop is ≥ `HUM_DROP_BUDGET_TENTH_DB`, the 300 Hz loss ≤
    `HUM_TONE_LOSS_BUDGET_TENTH_DB`, and `hum_removal_magnitude_db` matches the measured response at
    50 / 100 / 150 / 300 Hz within **0.1 dB**; printed as `AU5_HUM` — media `tests/au5_fixtures.rs`.
A9. §3.11(c): the corruption is `0.9 × (−1)^(k + j)`, alternating **per frame** (R31), occupying
    frames `91_200..91_208` for the last click; `click_count == 19` on `corrupt` and `== 0` on
    `clean`; the RMS-to-RMS error drop is ≥
    `DECLICK_ERROR_DROP_BUDGET_TENTH_DB` with the quantity named in the print; the 5 ms exponential
    burst reports zero clicks and is returned within **1e-6**; printed as `AU5_DECLICK` — media
    `tests/au5_fixtures.rs`.
A10. **S14**: `Analysis::audio_repair` reports the 10th/90th percentiles by `nearest_rank_index`,
     `None` below ten energetic windows with a `low_window_count` finding, hum excess summed over four
     harmonics against shoulders at `f × 2^(±1/6)` over ≥ 4 800-sample Goertzel blocks with the four
     per-harmonic values reported, and `click_density_per_minute` from the same `detect_clicks` the
     node uses; a synthetic 50 Hz-only fixture reads a large `hum_50` and a near-zero `hum_60` — media
     `export.rs`, `tests/au5_fixtures.rs`.
A11. **S12**: §3.11(d)'s Part A SNR-gain lane measures `get_audio_repair`'s `snr_db_hundredths` before
     and after on fixture (a) through a bus and asserts the gain ≥
     `AUDIO_REPAIR_SNR_GAIN_BUDGET_HUNDREDTHS` with ≥ 2× margin, printed as `AU5_REPAIR_SNR` — media
     `tests/au5_fixtures.rs`.
A12. Identity and neutrality: every node at its neutrals returns its input **bit for bit** through
     `process_buffer_static`; `bypass = 1` does the same and does **not** change the declared
     lookahead; a repair-free document's `mix_audio` is `to_bits()`-identical to the pre-AU5 render
     with a printed `AU5_REPAIR_SHA256`; and an `audio_denoise` at `reduction_tenth_db = 200` with all
     31 bands at the neutral is **unity**, not a mute — media `audio.rs`.
A13. **S10**: `has_gain_computer` is `pub` in core, re-exported, true for five names including
     `audio_denoise`, and **neither** `audio.rs` nor `mixer_pane_ui.rs` still declares a copy
     (asserted by an `include_str!` count over both files); `take_gain_reduction` returns the
     denoiser's `minimum_gain` and `None` for hum and de-click — core `tests/au5_core.rs`, media
     `audio.rs`, app `mixer_pane_ui.rs`.
A14. Both paths: `assert_playback_matches_export` at **1e-6** on a repair-bearing fixture **including
     its frame-5 seek arm**, asserted on a chain that is **not** retargeted mid-stream (R38); a
     learned profile written through `UpsertAudioBus`, read back, and producing the identical bin
     floor table after a retarget that preserves state; a **new arm** asserting the
     `direct_switch_remaining` window is exactly `window − 1` frames long and never dips, so the only
     two states that can differ between a fresh and a retargeted runtime are both pinned; and a chain
     at
     15 ms accepting a 5 ms limiter to land at exactly 20 while one at 21 is refused with
     `AudioBusLookaheadExceeded` — media `audio.rs`, core `tests/au5_core.rs`.
A15. `get_audio_repair`: description under **1 024 B** with the percentile clause in the **first**
     sentence; `deny_unknown_fields` refuses a misspelled field rather than defaulting it;
     `expected_revision` resolves to the uniform `stale_revision`; the range rule matches
     `get_audio_levels`; structured content is `{ timeline_revision, report }` and
     `assert_integer_leaves` passes over it — agent `server.rs`, `tests/mcp_server.rs`.
A16. `render_effects` spells a learned profile as one `noise_profile=[…]` block and **omits** it when
     every band is at the neutral, pinned by the carries/omits golden pair; **every pre-AU5 golden is
     byte-unchanged**; `effect_documentation` contains the profile pattern sentence, contains **no**
     `profile_band` row, reads its bounds from the descriptor, and the five spliced tool descriptions
     grow by the measured figure recorded in §0 — agent `render.rs`, `schema.rs`,
     `tests/mcp_server.rs`.
A17. **S12 and F19**: the three `BaselineProofAnalysis` arms each have a forwarding test against a
     typed-error double; and `plan_clip_fades` proposes the **same fades on the same fixtures** after
     its two-render hack becomes one `mix_window_levels` call, with the AU5-debt comment deleted and
     `au4_plan_clip_fades_measures_the_real_mix_and_commits_set_clip_audio` green — app
     `color_qc_ui.rs`, agent `server.rs`, `tests/mcp_server.rs`.
A18. Registry Part A: **81** hand-written capabilities, **135** registry, **54** generated; the
     ordering assert extended; the served quad **byte-identical**; the four byte asserts regenerated
     and the derivation comment's per-part split summing exactly; one M36 row; `qa_document` raises
     `"noise_profile_missing"` at `Warning` for a reducing denoiser with no profile and **not** for
     one at `reduction_tenth_db = 0`, and `export_ready` is unaffected — agent `schema.rs`,
     `server.rs`, `tests/mcp_server.rs`, core `qa.rs`.

### Part B (B1–B15)

Closes exit-gate clause 2 verbatim — *"fills are seamless at 1e-4 across the join"* (B4–B6) — and the
row's remaining deliverables: capture, fill and the two planners (§1.2).

B1. The room-tone store writes `<project stem>.kinewright-assets/room-tone/<sha256>.wav`, reads the
    length from **file metadata before a byte is read**, refuses over `ROOM_TONE_MAX_FILE_BYTES`,
    refuses a capture under `ROOM_TONE_MINIMUM_CAPTURE_MILLISECONDS` by name, and round-trips a
    written file through `probe_path` — media `room_tone_store.rs`.
B2. `capture_room_tone` decodes the **source** range at 48 kHz stereo, writes the store, probes,
    submits exactly **one** `AddAsset` carrying a `name` and a duration of at least one 30 fps frame,
    raises the write confirmation, and on a second identical call returns the same `asset_id` and
    emits **no** operation — agent `server.rs`, `tests/mcp_server.rs`.
B3. **S9**: `Document::track_gaps` returns leading and interior gaps and **never** the trailing one,
    is content- and kind-agnostic (a title closes a gap), and is unit-tested over a head gap, an
    interior gap, a trailing gap, adjacent clips, an **empty** track, a track whose only clip starts
    at 0, **and an unknown `TrackId`, which answers `None` rather than panicking or answering an
    empty list** (R42); and `qa_document`'s whole `QaReport` is **equal before and after** the refactor on that corpus
    and on qa.rs:684's existing case — core `qa.rs`, `tests/au5_core.rs`.
B4. **The seam.** §5.4's single-tile pin holds at `ROOM_TONE_SEAM_BUDGET = 1.0e-4` with a measured
    deviation of **exactly 0.0** and `margin=unbounded` printed as `AU5_ROOM_TONE_SEAM`; the doc
    comment states that exactness is a property of the **48 kHz PCM** fixture and does not generalise
    to a rate-converted source — media `tests/au5_fixtures.rs`.
B5. The multi-tile arm fills a 90-frame gap from the 60-frame sample and asserts **all three** joins
    — clip A → tile 1, tile 1 → tile 2, tile 2 → clip B (R46) — to the same budget; the tile count is capped at `ROOM_TONE_MAX_TILES` and a gap needing more
    is skipped with a reason — media `tests/au5_fixtures.rs`, agent `server.rs`.
B6. **S8**: `map_project_duration_to_source` round-trips every duration in `1..=120` at 30 → 30,
    30 → 25 and 30 → 24; the **25 fps** arm fills a 7-frame gap from source `0..8` with
    `clip_duration(fill) == 7` and the seam holding; the **60 fps** arm skips a 7-frame gap with rule
    98's reason while the rest of the plan commits; and no arm ever produces `ClipOverlap` — core
    `time.rs`, media `tests/au5_fixtures.rs`, agent `tests/mcp_server.rs`.
B7. `plan_room_tone_fill`: per-gap `{start, end, tiles, skipped_reason}`; a gap under
    `minimum_gap_frames` is skipped; a missing room-tone asset is named rather than assumed; it raises
    **no** confirmation; it is **idempotent** — a second run on the committed document proposes
    nothing; and `deny_unknown_fields` refuses a misspelled argument — agent `server.rs`,
    `tests/mcp_server.rs`.
B8. `plan_dialogue_repair` direct tests: the chain it builds is denoise → hum → de-click at the
    **head** of the bus with rule 108's parameters and `chain_lookahead_milliseconds == 15`; an
    existing bus is **reused** with its effects preserved; a chain that would exceed 20 ms is refused
    by name before `UpsertAudioBus` can fail; and `replace: false` against an existing repair prefix
    refuses with its text — agent `server.rs`.
B9. **S6**: on an unanalysed document `plan_dialogue_repair` refuses with text **containing**
    `"silence analysis is not ready"` and requests detection — the real message is
    `"asset {id} silence analysis is not ready: {…}"` (server.rs:8567-8577), so the assertion is
    `contains`, never `assert_eq!` on the whole string (R46) — and that refusal is a **different**
    string from the one for a document with silence analysis complete and no span reaching 469 ms; the real-engine test
    tolerates exactly one readiness refusal on the
    `au4_plan_audio_ducking_converges_through_the_real_engine` template — agent `server.rs`,
    `tests/mcp_server.rs`.
B10. **The measured refusal, which is the deliverable.** The real-engine test invokes
     `plan_dialogue_repair` on fixture (a) through the real engine, commits, re-measures through
     `get_audio_repair`, and asserts the SNR moved by ≥ `PLAN_REPAIR_SNR_GAIN_BUDGET_HUNDREDTHS` with
     ≥ 2× margin printed; a **negative arm on clean material** asserts the planner **refuses**, names
     both numbers, and prepares no plan — agent `tests/mcp_server.rs`.
B11. **S7**: `plan_dialogue_repair` and `plan_audio_normalization` run **in either order** land the
     same one bus — repair prefix, then compressor/gain, then true-peak limiter — with
     `chain_lookahead_milliseconds == 20` and the integrated loudness inside AU3's tolerance;
     `normalization_context` returns *extend bus N* only when every effect is an AU5 repair node
     **and** the bus's `tracks` equals the requested set, refusing a bus carrying one `audio_gain` and
     a bus whose tracks differ; a **non-zero bus fader and a `gain_curve` survive both orders**
     (R35), which the neutral-fader fixture alone would not catch because `gain_tenth_db` is
     `skip_serializing_if = "i32_is_zero"`; the in-crate refusal pin (server.rs:26140-26144) is updated; and AU3's
     g2/g3 eval baselines are unchanged — agent `server.rs`, `tests/mcp_server.rs`.
B12. Registry Part B: **84** hand-written capabilities, **138** registry, **54** generated; ordering
     assert extended; three descriptions under **1 024 B** each with their load-bearing clause in the
     first sentence; the served quad **byte-identical**; the four byte asserts regenerated with the
     per-part split summing exactly; three M36 rows — agent `schema.rs`, `server.rs`,
     `tests/mcp_server.rs`.
B13. Cards: `INSERTABLE_AUDIO_EFFECTS` is `[&str; 9]`; the replaced `debug_assert` fails when a name
     is not `is_audio_effect` or has no descriptor; `card_body` skips the 31 profile rows and
     `bypass`; `harmonic_count` reads `MixerUnit::Plain` and `notch_q_hundredths` reads **Q**, not
     Ratio; the denoise card paints a `reduction_bar` and the other two do not; and a headless painted
     frame of each card **writes no operation**, printing the offending ops if it does — app
     `mixer_pane_ui.rs`.
B14. Wells and heights: the noise well's bar geometry and its `"No profile learned."` state are pure
     functions tested without a window; the comb is `eq_well` with a parameterised magnitude source
     and matches `hum_removal_magnitude_db` at its 96 sample points; the **four new** ±1 px pins of
     rule 131 hold with their `MEASURED` constants recorded in §0; the master pane's **398 / 420** pin
     is unchanged; and `the_chain_pane_with_an_automation_section_still_fits_the_dock` passes with a
     repair-bearing bus — app `mixer_pane_ui.rs`, `mixer_ui.rs`.
B15. Gestures and docs: one `Learn` completion pushes **exactly one** operation under the existing
     `audio_bus:{id}` coalesce key and a single `Command::Undo` restores the pre-gesture document; the
     two refusal sentences are distinct and painted; the `Room tone` button is disabled with no
     selection and with no gap, and its `DoBatch` is one undo entry; DESIGN.md contains §6.5's Mixer
     and Timeline phrases and the existing `include_str!` slices still pass; `CHANGELOG.md`, the
     `ROADMAP-AND-WORKFLOWS.md` status paragraph naming the two parts and the clause split, README
     line 36 and the M36 rows are present — app `mixer_ui.rs`, `timeline_ui.rs`, docs.

Exit gate for each part: its checklist green in `cargo test --workspace` locally on Linux, `cargo
fmt --check` and `cargo clippy --workspace --all-targets -- -D warnings` clean, both CI operating
systems green after push, and both review passes recorded on every crate the part touches, core
included. Part B additionally needs one hands-on session on a real project — a profile learned on a
noisy location interview and heard, a hum notch checked against its comb, and one
`plan_dialogue_repair` committed and listened to — read by Riel. Part A lands as **`feat: complete
AU5a repair nodes and measurement`**; Part B as **`feat: complete AU5b room tone and repair
planners`**.

## 8. Files

**Part A.** *Core:* `effect.rs` (the three `EffectDescriptor` literals and their 45 rows, twelve
`EffectUniform` variants, `is_audio_effect` → eleven names, `is_static_audio_parameter`'s **two
edits** — the widened `AUDIO_LOOKAHEAD_PARAMETER` arm and one guarded arm before `_ => false` (R36) —
`is_noise_profile_parameter`, `has_gain_computer` hoisted); `operation.rs`
(`is_hold_only_parameter`'s `harmonic_count` at :4036-4038, `validate_audio_chain_automation`'s
amended reason string at :4835); `qa.rs` (`noise_profile_missing`); new `audio_repair.rs` (`AudioRepairRequest`,
`AudioRepairReport`, `AudioRepairProvenance`, `AudioRepairMeasurements`, `audio_repair_exceptions`,
the five thresholds); `media.rs` (`MixNoiseProfileRequest`, `NoiseProfileReport`, `MixWindowRequest`,
`MixWindowLevelReport`, the three `Analysis` methods); `lib.rs` (re-exports); `tests/contracts.rs`;
`tests/au2_core.rs` (the reason string at :634); new `tests/au5_core.rs`.
*Media:* `spectrum.rs` (`inverse_fft`, `stage_twiddles`, `STAGE_TWIDDLES`, `hann_window` and
**`SILENCE_POWER` → `pub(crate)`** (R34), and **`third_octave_spectrum` parameterised by
`(segment_frames, hop_frames, minimum_frames)`** with `normalised_length()`, `hann_window`, the
`power` vector, the `scratch` buffer and `normalisation` threaded through it — AU2's callers pass
today's `SPECTRUM_*` constants and their goldens are pinned byte-unchanged (R33));
`loudness.rs` (`nearest_rank_index` → `pub(crate)`); `audio.rs`
(`AudioEffectState::{Denoise, HumRemoval, Declick}`, `DenoiseState`/`HumState`/`DeclickState`, the
three `AudioEffectRuntime::new` arms, the three `process_frame` arms, `take_gain_reduction`'s
`Denoise` arm, `node_lookahead_milliseconds`, `hum_removal_magnitude_db`, `detect_clicks`, the
deleted private `has_gain_computer`, the four promoted test helpers' call sites, whose **signatures
do not change** — `pseudo_random` at :5363-5365 and every AU2/AU3 unit test still compile (R45));
`test_support.rs` (`tone`, `pseudo_random_amplitude`, `rms`, `wav_f32`); `export.rs`
(`measure_mix_noise_profile`, `measure_mix_window_levels`, `measure_audio_repair`, and
`measure_mix_spectrum` passing the `SPECTRUM_*` constants explicitly into the parameterised
`third_octave_spectrum`); `lib.rs`; new
`tests/au5_fixtures.rs`. *Agent:* new `audio_repair_tool.rs`; `schema.rs`
(`INSPECTOR_TOOL_NAMES` 80 → 81, `effect_documentation`'s filter,
`noise_profile_pattern_documentation`); `render.rs` (the `noise_profile` arm and two goldens);
`server.rs` (the handler, its dispatch arm and row, the ledger figures and derivation comment,
`plan_clip_fades`' rewritten measurement, `NoopMedia`'s three doubles); `tests/mcp_server.rs`.
*App:* `color_qc_ui.rs` (three `BaselineProofAnalysis` arms and three forwarding tests);
`mixer_pane_ui.rs` (the deleted private `has_gain_computer`). *Docs:* this file, `CHANGELOG.md`, `MEDIA-POLICY.md`,
`AU2-EQ-AND-DYNAMICS.md` (**two** §0 errata — R6's reason string and R40's narrowing of the bypass
rule for `audio_denoise` — and the §2.2 string), `AU3-LOUDNESS-AND-DELIVERY.md` (pointer),
`M36-AGENT-RUNTIME-EFFICIENCY.md`.

**Part B.** *Core:* `model.rs` (`Document::track_gaps(&self, track: TrackId)
-> Option<Vec<Range<TimeCode>>>`, R42); `time.rs`
(`map_project_duration_to_source`); `qa.rs` (the `track_gaps` refactor); `lib.rs`;
`tests/au5_core.rs`. *Media:* new `room_tone_store.rs` (`RoomToneStore`,
`ROOM_TONE_STORE_DIRECTORY`, `ROOM_TONE_MAX_FILE_BYTES`, `ROOM_TONE_MINIMUM_CAPTURE_MILLISECONDS`,
`ROOM_TONE_MAX_CAPTURE_MILLISECONDS`); `lib.rs`; `tests/au5_fixtures.rs` (the seam lanes).
*Agent:* `schema.rs` (`INSPECTOR_TOOL_NAMES` 81 → 84, three descriptions); `server.rs`
(`capture_room_tone`, `plan_room_tone_fill`, `plan_dialogue_repair`, their argument structs, dispatch
arms and rows, `normalization_context`'s relaxation, `normalization_bus`'s append signature, the
updated in-crate refusal pin at :26140, the ledger figures); `tests/mcp_server.rs`.
*App:* `mixer_pane_ui.rs` (`INSERTABLE_AUDIO_EFFECTS` 6 → 9 **and both "six" doc comments at :1602
and :1616** (R46), the replaced `debug_assert`,
`effect_display_name`, `parameter_label`, `mixer_unit`'s amended doc, `insert_audio_effect`'s profile
skip, `noise_well`, `eq_well_magnitudes`' parameterised source, the `Learn profile` button, the four
height pins); `mixer_ui.rs` (`MixerFrame.learn_noise_profile: Option<(AudioChain, EffectId)>`, `Copy`-preserving
per R43; the DESIGN.md pins; the dock test);
`app.rs` (the single-flight learn worker, its `poll_background` arm and 50 ms repaint, the one chain
set it pushes); `timeline_ui.rs` (the `Room tone` button, its enablement rule, its `DoBatch`, the
DESIGN.md pin); `project.rs` (only if the button needs session state; it does not today).
*Docs:* this file, `DESIGN.md`, `CHANGELOG.md`, `README.md`, `ROADMAP-AND-WORKFLOWS.md`,
`MEDIA-POLICY.md`, `AU3-LOUDNESS-AND-DELIVERY.md` (pointer), `M36-AGENT-RUNTIME-EFFICIENCY.md`.
