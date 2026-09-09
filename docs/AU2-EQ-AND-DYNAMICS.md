# AU2 EQ and dynamics

Status: draft for critic review (2026-09-08, revision 3). Built from the AU2 design brief
(D1–D12) as amended by its **Addendum** (R1–R28 on `critic-brief.md` F1–F28), by
`critic-contract.md` F1–F34 (rulings A1–A34), and by `orchestrator-notes.md` N1–N5, against
main 6426449. Where a later ruling and an earlier one conflict, the later wins; N5 overrides
`critic-contract.md`'s own ruling wherever they differ. The numbers in this document are the
contract; implementation follows them or amends this file first.

AU2 is the second slice of the audio programme (ROADMAP-AND-WORKFLOWS.md:646). It lands as
**one contract in two parts, two commits** (R27): **Part A — EQ and dynamics nodes**
(descriptors, DSP, document-derived latency, gain-reduction telemetry, agent prose; no new
operation, no new document field, no UI) and **Part B — Bus and master control** (bus fader,
master chain, pan law, live editing in the Mixer dock, the spectrum tool). Part A lands first.
Each part runs the full workspace gate and both review passes.

## 0. Errata

The implementation records each deliberate deviation from the text below here, as AU1 §0
does. Two figures are regenerated as the code lands: Part A's `description_bytes` total and
Part B's registry totals.

**Part A, core (2026-09-08):**

- **E1. `bypass` row position.** §2.1 fixes the values of the `bypass` row, not its position.
  On the five pre-AU2 descriptors `AUDIO_BYPASS_DESCRIPTOR` is the **last** row, so their
  existing row order (and the order parameters appear in the agent's generated prose) is
  untouched, matching `COLOR_NODE_BYPASS_DESCRIPTOR`'s position in `COLOR_WHEELS_PARAMETERS`;
  on the three new descriptors it is the **first** row, as their tables show.
- **E2. `AUDIO_BYPASS_DESCRIPTOR` is crate-private**, as the §2.1 snippet spells it; A1 pins
  "one shared descriptor" by record equality across the eight `bypass` rows.
- **E3. `AUDIO_LOOKAHEAD_PARAMETER`**, a `pub(crate)` const in `effect.rs` holding
  `"lookahead_milliseconds"`, keeps the string from being re-spelled across three modules. Not
  part of the public surface.
- **E4. `AudioBusLookaheadExceeded`'s message** interpolates `CHAIN_LOOKAHEAD_MILLISECONDS`
  rather than the literal `20`; the rendered string is byte-identical to §2.3's and A7 pins it.
- **E5. Check order in `validate_audio_bus`.** The §2.2 rule-2 rejection runs before
  `curve.validate()`, so any `keyframes` entry on `lookahead_milliseconds` — structurally
  invalid or `Hold`-only alike — reports `reason: "sets processing latency and cannot be
  keyframed"`. §2.2 says "any".
- **E6. Placement.** `is_static_audio_parameter` lives in `effect.rs` beside `is_audio_effect`;
  `Effect::static_integer_parameter`, `CHAIN_LOOKAHEAD_MILLISECONDS`, `ChainLookahead`,
  `chain_lookahead_milliseconds`, and `AudioMix::lookahead_milliseconds` live in `model.rs`
  beside `AudioMix`. All are re-exported from `lib.rs` beside the AU1 names.

**Part A, agent (2026-09-08):**

- **E7. Regenerated figures.** Registry `serialized_bytes` 1,303,967 → **1,312,132 B**;
  `description_bytes` 96,840 → **105,005 B** (+8,165); `input_schema_bytes` asserted unchanged at
  1,186,449 B; tool count 126 / inspectors 76 unchanged; served triple 7 / 5,660 / 3,510 / 998
  asserted byte-identical in `server.rs` and `tests/mcp_server.rs`.
- **E8. A18 is two tests**, `au2_audio_bus_prose_names_the_new_nodes_and_the_chain_budget` and
  `au2_parametric_eq_band_rows_are_one_pattern_not_twelve_entries`, because one test tripped
  `clippy::too_many_lines`; together they cover every A18 assertion including the negative
  ones (no `band{1..4}_*` name in any of the five effect tools' descriptions; no "ITU").
- **E9. Pattern sentence wording.** `audio_parametric_eq_pattern_documentation()` emits
  `band{i}_hertz/band{i}_gain_tenth_db/band{i}_q_hundredths for i=1..=4, one peaking band each:
  hertz=20..=20000, neutral 120/500/2000/8000 by band; gain_tenth_db=-240..=240, neutral 0;
  q_hundredths=10..=1800 hundredths of Q, neutral 71` — every bound read from the descriptor; a
  parenthesised decimal gloss was dropped because the description parser stops at the first `)`.
- **E10. `server.rs` pins three extra figures** (`input_schema_bytes`, `description_bytes`, and
  the four-field served tuple) beside the existing `(serialized, served_serialized)` pair.
- **E11. Where the +8,165 B goes.** §4.2's "only the five effect tools' descriptions grow"
  overlooks §4.1's own rewrite: the forty descriptor rows add 1,299 B to each of the five tools
  carrying `effect_documentation()` (6,495 B), and the rewritten bus arm grows
  `upsert_audio_bus` and `remove_audio_bus` from 335 B to 1,170 B each (1,670 B). The arm
  gained, after the second-pass review, the closed set of legal names ("Bus effects must use one
  of audio_gain, … or audio_true_peak_limiter.") because `upsert_audio_bus` carries no
  `effect_documentation()` and the arm is the only place an agent can read them; the schema
  test pins each name against `is_audio_effect`. The split is stated in `server.rs`,
  `tests/mcp_server.rs`, and M36. `is_parametric_eq_band_parameter`
  matches exactly `band` + one digit 1–4 + `_`, and the schema test pins that bands 2–4 share
  band 1's bounds and that the sentence's centre list is rebuilt from the four descriptors.

**Part A, media (2026-09-08):**

- **E12. The true-peak estimate folds in the same frame's sample peak.** The 4× phase grid
  (`(n − 23.5)/4`) is offset by one eighth of an input sample, so an isolated impulse reads
  about 0.23 dB under its own sample and the node would have emitted a sample peak above the
  ceiling (A11's impulse case). The node uses `max(phase estimate, |x[i − D]|)`, which bounds
  the sample peak exactly and the inter-sample peak to within the interpolator's error (E34
  measures +0.20 dB on broadband noise, inside the ±0.3 dB budget). `TruePeakEstimator::
  estimate()` stays phases-only so the `fs/4` pin still measures the contract's detector.
- **E13. Tail truncation runs after the `keep_from` drain**, head drops before it; truncating
  to `(end − start) × channels` before draining `start` would leave `T − 2S` samples. Final
  lengths are exactly as §3.7 states.
- **E14. `MixMeters::matches_document` is a Part B addition** (its only caller is §5.8); an
  uncalled `pub(crate)` fn fails `-D warnings`.
- **E15. `AudioChain` and `MixPeaks.gain_reduction` live in core `media.rs`** and `AudioChain`
  is re-exported from core `lib.rs`. `MixPeaks` is not serialized (derives `Debug, Clone,
  Default, PartialEq`), so no serde attributes apply to the new field.
- **E16. The compressor's RMS window length is fixed at construction** from
  `static_integer_parameter("rms_window_milliseconds")` (neutral 10); `detector` is still read
  per sample frame and the window advances only while `detector == 1`. A curve on
  `rms_window_milliseconds` would therefore be silently ignored, so core makes it the third
  static pair: `is_static_audio_parameter` is true for exactly
  `("audio_compressor" | "audio_true_peak_limiter", "lookahead_milliseconds")` and
  `("audio_compressor", "rms_window_milliseconds")`; a curve on the RMS window is rejected with
  `InvalidEffectAutomation { reason: "is read once when the chain is built and cannot be
  keyframed" }` while the lookahead pair keeps `"sets processing latency and cannot be
  keyframed"`. `chain_lookahead_milliseconds` filters on the lookahead name explicitly and does
  not count the window (A7 pins a `detector = 1, rms_window_milliseconds = 100` compressor at
  0 ms). The per-effect keyframe checks moved into a private
  `validate_audio_bus_automation(duration, bus, effect)` to keep `validate_audio_bus` under the
  pedantic line limit; check order is unchanged.
- **E17. Gate and compressor identities are pinned at 48 kHz**, where `smooth_gain`'s
  coefficient is ≥ 0.5 and `1 − c` is exact; A27's short-circuit is not added to the gate so
  `smooth_gain` stays unchanged. Only the true-peak limiter carries the structural
  short-circuit and is pinned at rate 1 000 too.
- **E18. A9's stored expectation is an inline AU1 reference implementation** (the AU1 gain
  computer plus `smooth_gain`) asserted with `==`, which also pins `smooth_gain`.
- **E19. A16's `measure_mix_levels` test lives in `audio.rs`'s test module** beside the existing
  stem test; `export.rs`'s test module is video-only.
- **E20. `smooth_gain`'s arguments are renamed `falling_milliseconds` / `rising_milliseconds`**
  (behaviour unchanged); `engine.rs` needed no change because telemetry flows through the
  existing `MixMeters::peaks()`.
- **E21. A19 figure.** Worst-case chain (10 ms true-peak limiter + 10 ms RMS lookahead
  compressor) at 48 kHz stereo cost **11.6 ms per second of audio in release** before E32 (0.012×
  real time; 103.6 ms in debug), so `LIVE_FILL_MILLISECONDS = 1000` needs no revisiting. New module
  `crates/kinewright-media/src/dsp.rs` holds the biquad, delay line, true-peak estimator,
  sliding minimum, boxcar, and mean-square window; `parametric_eq_magnitude_db` is public from
  `kinewright_media` for Part B's EQ well; `GeneratedMedia::from_bytes` was added to
  `test_support`.

**Part A, second pass (2026-09-08):**

- **E22. `chain_lookahead_milliseconds` treats a non-`Integer` stored value as absent.** It
  is unreachable behind `validate_effect` (audio parameters are always `Integer`); recorded,
  not changed.
- **E23. Parameter-name constants.** Core's `AUDIO_LOOKAHEAD_PARAMETER` and
  `AUDIO_RMS_WINDOW_PARAMETER` are crate-private; the descriptor rows and the media crate spell
  the literals themselves. E3's "three modules" is two core sites plus the descriptor table.
- **E24. §3.6's quoted MEDIA-POLICY block was edited to match the document** (backticks on `k`
  and `target + k`, "so every project" for the em-dash); the two are byte-identical.
- **E25. §2.2's "true exactly for" two pairs is superseded by E16's three**; the sentence is
  left in place with this pointer rather than rewritten, as AU1 §0 did for its amendments.
- **E26. `D` is fixed at construction.** The true-peak group delay `D` is 6 when the node is
  built with `true_peak = 1` and is not reset to 0 if `true_peak` is later held to 0 by a
  keyframe; the sample-peak estimate is then simply stored `D` frames late, which keeps the
  ceiling bound (the window still contains the emitted frame). §3.5's "0 for sample peak"
  describes the construction-time choice.
- **E27. A3's reach.** The empty-parameters-equals-all-neutral sweep cannot detect a wrong
  hard-coded lookahead neutral (the per-chain pad absorbs it) or a wrong RMS-window neutral
  (`detector`'s neutral is 0). A1's descriptor table test and A7's `chain_lookahead` pins cover
  the lookahead neutral from the core side.
- **E28. `parameter_epoch` is never bumped in Part A** because Part A has no live retarget
  path for chain parameters; Part B's `update_audio_mix` structural diff (§5.8) bumps it and
  B11 pins the next-sample-frame audibility.
- **E29. A19's cost test records, it does not regress-detect**: it asserts a loose upper bound
  and prints the measured figure (E21). A tighter bound would be machine-dependent.
- **E30. Bypass runs the state machine.** The first media review found that a bypassed
  dynamics node froze its detector, so un-bypassing a limiter emitted `Lh` frames the detector
  had never analysed (measured +5.5 dB over the ceiling on a replica). Every dynamics node now
  advances detector, envelope/RMS window, sliding minimum, boxcar, release state and delay while
  bypassed and skips only the output multiply and the §3.8 telemetry update (the two filter
  nodes, `audio_eq` and `audio_parametric_eq`, freeze their state under bypass, as A6's
  "bit-identical to its delayed input" requires of a filter); pinned by
  `un_bypassing_a_limiter_mid_signal_never_breaks_its_ceiling` and
  `un_bypassing_a_compressor_lands_on_a_live_envelope`.
- **E31. Two-way `r` identity.** A27's short-circuit was one-way (`r` settled at the rounding
  fixed point `1 − ulp/(2(1−c))` below unity after any limiting, so telemetry never returned to
  0 dB). When `m[i] >= 1.0`, `r` now snaps to exactly `1.0` when the release update lands within
  `RELEASE_IDENTITY_EPSILON = 2^-20` of unity or stops climbing; the snap happens only while
  `m >= 1`, where `required[d] == 1` for every covered frame, so the ceiling proof is untouched.
  `Boxcar` keeps a unity counter and returns exactly `1.0` when every slot is `1.0`. Pinned by
  `gain_reduction_returns_to_exactly_zero_after_the_release` at 48 kHz and rate 1 000.
- **E32. `bypass` is read through the per-project-frame cache** (`bypass_cache`, keyed on
  project frame and parameter epoch), so the pre-AU2 chunk path carries no new per-sample map
  probe; the A19 figure improved to **8.0 ms per second of audio in release** (83.7 ms debug).
- **E33. `mix_pass` enumerates programme audio to `range.end + L`** (clamped to the document
  duration) so a sub-range measurement's flush frames are limited against real audio, not
  silence; unchanged when `L == 0` or `range.end == duration`. Pinned by
  `a_sub_range_measurement_limits_against_the_real_programme`.
- **E34. Broadband true-peak overshoot** measured with the independent 16× interpolator: noise
  at ×1.8 through a −1.0 dBFS ceiling reads **+0.20 dB** over, inside OPEN-4's ±0.3 dB budget;
  added to A11 and printed under `--nocapture`.
- **E35. Second-pass media fixes.** `mix_pass` derives its segment enumeration end from the
  last mixed sample frame (`samples_to_frame(total − 1) + 1`, clamped to
  `[range.end, duration]`), so the pre-AU2 path enumerates exactly `range.end` frames again
  (pinned at four points by `the_mix_pass_segment_enumeration_covers_only_the_mixed_frames`);
  and the compressor resolves `knee_tenth_db` and `detector` once per project frame through the
  same parameter-epoch cache as `bypass`, so a pre-AU2 compressor makes exactly AU1's five
  per-sample parameter reads. The A19 figures of record are E32's (8.0 ms release); E21's 11.6 ms
  predates the cache.

**Part B, core (2026-09-08):**

- **E36. One chain-automation validator.** `validate_audio_bus_automation` became
  `validate_audio_chain_automation(chain: AudioChain, …)`, reusing the public
  `media::AudioChain { Bus, Master }` so the hold-only, never-keyed, and value-range checks are
  one code path for buses and the master; only the project-range rejection branches on the
  chain. Check order and bus behaviour unchanged.
- **E37. `validate_audio_master(doc, master)` takes the candidate**, mirroring
  `validate_audio_bus(doc, bus)`, so the operation validates before storing; `validate_audio_mix`
  passes `&doc.audio_mix.master`. Both private.
- **E38. Bus gain is checked right after the `InvalidAudioBus` name/tracks check**, before the
  track and effect loops. `SetPanLaw` stores inline in `apply_unchecked` (nothing to validate);
  `SetAudioMaster` has `set_audio_master`.
- **E39. Literal counts.** Part A had grown `AudioBus { … }` to 24 literals (not §5.1's 16) and
  four `AudioMix { … }` literals also needed the two new fields; the `AudioMix` sites took
  `..AudioMix::default()`.
- **E40. Extra accessors and consts.** `AudioMix::bus(id)`, `AudioMix::bus_for_track(track)`,
  `AUDIO_BUS_GAIN_MIN/MAX` and `AUDIO_MASTER_GAIN_MIN/MAX` (−600 / 120, pinned equal to the
  `audio_gain` descriptor and to `TRACK_MIX_GAIN_MIN/MAX`). The spectrum types derive `Eq`
  (all-integer fields). `operation.rs:100`'s "Balance law" doc comment is the agent
  implementer's edit (it is tool-description text).
- **E41. Message formats.** `VisualEffectOnAudioMaster` renders the effect name with `{:?}`
  (quoted), matching its `VisualEffectOnAudioBus` sibling rather than §5.4's `{effect}`; the two
  gain-range messages interpolate `AUDIO_BUS_GAIN_MIN/MAX` and `AUDIO_MASTER_GAIN_MIN/MAX` in
  E4's pattern. `PanLaw::is_balance` carries no `trivially_copy_pass_by_ref` allow because the
  lint does not fire on a `pub` method (avoid-breaking-exported-api); §5.3's instruction to add
  one is withdrawn. The pre-AU2 load/save test builds its fixture from the pre-AU2 field set
  through serde rather than a checked-in JSON string.

**Part B, media (2026-09-08):**

- **E42. `PanLaw::Balance` keeps AU1's `f32` expression verbatim.** §5.7 shows both laws
  computed in `f64` and cast once. Evaluating AU1's `[1 - p.max(0), 1 + p.min(0)]` in `f64` and
  casting the result changes **82 of the 201** integer pan positions by one `f32` ulp (the first
  is `-99`, where the `f32` path lands on an exact tie and rounds to even while the `f64` path
  does not), which would change the exported bytes of every pre-AU2 document carrying one of
  them — exactly what §5.10(f) and A14 forbid, and what B8's "`Balance` is bit-identical to AU1"
  asks for. Only `ConstantPower` computes in `f64`; the cardinal special case is scoped to it,
  since AU1's own expression is already exact at `-100`, `0`, and `+100`. B8 pins all 201.
- **E43. The structural key excludes `rms_window_milliseconds`.** §5.8 defines structure as
  `(EffectId, effect name, static lookahead_milliseconds)`, so a live change to the
  compressor's RMS window retargets in place and keeps the window length the chain was built
  with. That is E16's own rule ("read once when the chain is built"), not a new limit; recorded
  because the window is the one static parameter the key does not carry.
- **E44. The fader survives a chain rebuild.** §5.8 lists the gain retarget (step 5) after the
  chain rules (steps 3–4), so when a chain is rebuilt its `GainRamp` is carried across from the
  old runtime and retargeted rather than reconstructed settled: only the chain's DSP state takes
  the momentary discontinuity a structural edit accepts.
- **E45. `third_octave_spectrum` enforces the two-segment minimum itself.** §5.9 puts the
  24 576-frame rejection at the caller. The analyser refuses the same length, so B13's
  "24 575 refused, 24 576 accepted" is pinned on the analyser directly and
  `measure_mix_spectrum` can still reject before it decodes anything.
- **E46. Bin extent for the overlap weighting.** §5.9 says each bin contributes in proportion to
  the fraction of its 2.93 Hz width inside the band without fixing where that width sits: bin
  `k` is taken to span `[k·Δf − Δf/2, k·Δf + Δf/2)`. `window_limited` and the main-lobe width are
  derived from the runtime rate rather than hard-coded, and the band's upper edge is clamped to
  Nyquist (a no-op at 48 kHz, where the 20 kHz band ends at 22.6 kHz).
- **E47. New media names.** `GainRamp` and `AudioMasterRuntime` in `audio.rs`;
  `AudioMixProcessor::master_stage_frames` beside `bus_stage_frames`;
  `AudioEffectRuntime::retarget`, `node_structure`, `node_lookahead_milliseconds`,
  `chain_structure_matches`, `retarget_chain`, and `gain_reduction_keys` as module-private
  helpers; `export::measure_mix_spectrum` plus the shared `clamped_measurement_range` and
  `measurement_settings` `measure_mix_levels` now uses too; `engine::can_retarget_audio_mix` and
  `engine::mix_meters_for_update`, extracted so A34's latency guard and A33's rebuild rule are
  pinned without standing up a worker. `AudioEffectRuntime::new` now derives its own
  `latency_frames` through `node_lookahead_milliseconds`, so the structural key and the delay
  line can never disagree. Nothing new is public from `kinewright_media`.
- **E48. B10 uses a sibling fixture.** `parity_document_with_master_chain` extends
  `parity_document_with_full_chain` with the master gain, the master true-peak limiter, a
  non-neutral bus fader, `PanLaw::ConstantPower`, and a panned track, so A17's existing
  `bus_stage: 10, master_stage: 0` pin is untouched. The new test asserts
  `master_stage: 5` and a 720-frame graph latency (480 + 240, the sum of the two truncations).
- **E49. The through-the-mix spectrum test lives in `audio.rs`**, for E19's reason: `export.rs`'s
  test module is video-only and every audio fixture (`wav_f32`, `audio_clip`, `audio_asset`) is
  in `audio.rs`. The pure-analysis pins are in `spectrum.rs`.
- **E50. B7's `decode_from == TimeCode::ZERO`** is observed as `mixer.sources[0]
  .project_sample_start == 0` — the mixer keeps no `decode_from` field — with the cursor landing
  at `target + L` per §3.7. The fixture's master limiter declares 5 ms, the descriptor's minimum
  being 1 rather than 0. A master carrying only a fader still needs no preroll, which the same
  test pins.

Remaining OPEN notes for the critic:

- **OPEN-3 (carried).** `lookahead_milliseconds` is read **once** at
  `AudioEffectRuntime::new` and may never be keyed (§2.2). Because `L` is derived from the
  document (R1), a live edit that changes any chain's lookahead sum cannot be applied to a
  running processor: the app predicate compares `AudioMix::lookahead_milliseconds()` of the old
  and new documents and falls back to the stop-and-re-cue path when they differ, and the worker
  compares them again and re-cues defensively (§5.8). Recorded as a limit (§6.10), not a
  deviation.
- **OPEN-4 (new, from N5/Q3).** The true-peak detector has the **structure** of ITU-R
  BS.1770-4 Annex 2 (48 taps, 4 phases of 12) with **this contract's own Blackman
  windowed-sinc coefficients**, not the ITU coefficient table. It is never called "the ITU-R
  BS.1770-4 Annex 2 detector" anywhere in the contract, the descriptors, or the agent prose.
  **BS.1770-4 true-peak conformance of the delivered file needs the ITU coefficient table and
  is deferred to AU3**, where the delivery check measures the written file.

Resolved since revision 2: OPEN-1 (bypass on all eight descriptors, one shared uniform per A8);
OPEN-2 (per-family head-and-tail stem trim, A2).

**Part B, agent (2026-09-08):**

- **E51. Registry figures.** 129 tools / 77 inspectors; registry **1,421,520 B** =
  **1,293,084 B** input schemas + **107,271 B** descriptions; served quad 7 / 5,660 / 3,510 /
  998 byte-identical. §4.2/§6.4's "input schemas cannot move" holds for Part A only: Part B
  changes the `Operation` model, so `input_schema_bytes` grows by 106,635 B: the two new
  mutators' own schemas (43,559 B), `get_audio_spectrum`'s own schema (1,340 B), the widened
  shared `$defs` in each of the fifty generated tools (49 × 1,195 B, plus 1,195 + 62 B on
  `set_track_mix` for its law-neutral doc comment), and `apply_edit_plan`, the one non-generated
  tool that embeds `Operation` (+1,924 B). Descriptions grow 2,266 B = 1,302 (two
  new mutators) + 636 (`get_audio_spectrum`) + 194 (bus fader sentence × 2) + 134 (`set_track_mix`
  rewrite).
- **E52. Test names.** B17's round trip is `au2_set_audio_master_and_pan_law_round_trip_through_
  edit_plans_and_state` and carries an `upsert_audio_bus` with `gain_tenth_db: -35`, pinning the
  ` gain=` field; B14's annotations live in `au2_master_and_pan_law_tools_are_documented_and_
  idempotent`, which also pins `remove_audio_bus` as neither idempotent nor destructive;
  `set_track_mix_schema_documents_ranges_and_the_balance_law` keeps its name and now asserts the
  old unconditional claim is gone.
- **E53. Handler details.** `get_audio_spectrum` checks the `track`/`bus` conflict before the
  range; `render_band_center_hertz` uses `is_multiple_of(10)` (clippy); the bus fader sentence is
  the second sentence of the bus arm; the `SetTrackMix.pan_percent` doc comment is at
  `operation.rs:111` after Part A's shift; `normalization_context` allocates through
  `AudioMix::next_bus_id()` (B5). Two test-only `#[allow(clippy::too_many_lines)]` were added;
  `AudioSpectrumArgs`' doc comments mirror `AudioLevelsArgs` rather than §6.2's literal wording
  (both true). The compact golden literal is hoisted into `const COMPACT_GOLDEN` shared by the
  AU1 golden test and the AU2 neutral-omission test, so the byte-unchanged claim is asserted
  against stored bytes.

**Part B, app (2026-09-08):**

- **E54. `mixer_body` returns `Option<MixerSelection>`** (§6.6 wrote `()`): the `Edit` toggle
  is a control that changes the selection, and a selection naming a bus the document lost is
  cleared. `mixer_panel` and `MixerHarness` assign it to their own field.
- **E55. `MixerChainEdits` carries `pan_law: Option<PanLaw>` and `gesture_started: bool`** in
  addition to `buses`, `master`, `live`; controls call `chain.begin_gesture()` and the fold calls
  `edits.begin_gesture()` once, keeping `InspectorEdits` out of the strip/pane signatures.
  `SetPanLaw` is folded under `audio_master` with the frame's live rule (a radio click is never
  a drag, so it is the discrete branch in practice).
- **E56. Control signatures.** `mixer_parameter_control` takes `chain: MixerChain<'_>` (the
  selection plus the chain's current value to clone); `meters_and_fader` takes range and value,
  since all three faders read in decibels through `format_gain_db`/`parse_gain_db`.
- **E57. Layout.** `+ Bus` shares the `Reset` row (its own row made the `NO AUDIO` strip 244 px);
  a collapsed card carries no `ui.group` frame (the row's whole budget is `ICON_BUTTON`); the
  pane is allocated at `MIXER_CHAIN_PANE_WIDTH` and each control gets a fixed cell of
  `(MIXER_CHAIN_PANE_WIDTH − space::EIGHT) / 3`, three per row — `set_max_width` alone does not
  hold a `ScrollArea`, and a nested `Ui` of unknown width defeats `horizontal_wrapped`, which a
  real screenshot (not a test) caught as an EQ card running off the window. Pinned by
  `the_chain_pane_is_one_column_however_wide_the_dock_is`. Measured: track strip 218 / 232 /
  232 px, bus strip 215 px (one node and six nodes + sidechain + gain), master 196 px, collapsed
  card 18 px, all under budget; DESIGN.md names these figures.
- **E58. `MixerUnit::Plain`** is a seventh, defensive formatter for a registered name in none of
  §6.7's five units; a test asserts no insertable node has one. Gain reduction is stored as a
  fraction of `MIXER_REDUCTION_METER_RANGE_DB` (which lives in `mixer_ui.rs`, a range not a
  size token) so it decays on the shared 0.9-per-second schedule; `MixerMeterLevels::reduction`
  returns decibels.
- **E59. Routing checkboxes also list a track already on this bus even if it carries no audio**,
  so a routed track can always be moved off. The test rect recorder is keyed by `String` with
  `record_keyed_rect`/`record_param_rect`; a flag control records only the unselected option's
  rect.
- **E60. B21's "no longer contains `read-only`" is pinned as the removal of the sentence "Bus
  strips are read-only…"**, because §6.9's own replacement text calls the EQ well a read-only
  magnitude well. `is_effect_insertable` and `effect_display_name` became `pub(crate)`; the
  latter gained the eight audio display names.
- **E61. Screenshot harness.** `KINEWRIGHT_SCREENSHOT_SHOW=mixer-chain` raises the Mixer tab and
  pre-selects the first bus (Master on a bus-free document) so a static capture shows the pane;
  `=mixer` is unchanged. Both verified by real captures.

**Part B, second pass (2026-09-08):**

- **E62. Media review notes.** E47's `AudioMixProcessor::master_stage_frames` is a field, not an
  accessor (export trims by `graph_latency_frames`). A structural rebuild of a chain, removing a
  bus, or un-routing a track drops that path's audio for `L_bus` frames (the pad refills from
  silence) — this is §5.8's "momentary discontinuity", now stated with its length. Master-chain
  automation reads `project_at` from the output index, so it acts up to `L_bus` early relative
  to content that arrives through a bus pad; playback and export agree. The worker's latency-
  guard fallback re-cues at the current position (second-pass fix); the FFT recomputes
  `sin_cos` per butterfly (measurement path only).
- **E63. App review notes.** A live bus-fader drag and an Enter-committed master readout in the
  same frame fold both operations under the frame's first live key (`audio_bus:{id}`); the
  journal keeps every batch, so only undo granularity is affected. The EQ well samples the
  chain at `TimeCode::ZERO`, so a keyframed EQ shows its t = 0 curve. Rounded readouts ("1.2 kHz"
  for 1250 Hz, "4.0:1" for 405) were re-parsed on focus-out and wrote the rounded value back;
  the parsers are value-aware (a text equal to the current value's own rendering returns the
  current value) — second-pass fix, pinned by harness tests.
- **E64. Pane strings and rounding.** `LAST_TRACK_REASON` reads "a bus keeps at least one
  track; ask the agent to remove the bus instead" (the pane has no bus-removal control; core's
  own message is unchanged), not §6.8's quoted string. Parsed pane values are rounded before
  they reach the integer `DragValue` (emath truncates `from_f64`), so "Q 0.57" is 57 and
  "2.3:1" is 230; the round-trip pin is exact. The card header tooltip shows the registered
  name and node id. The `Playback::update_audio_mix` trait doc comment now describes the AU2
  live set and the re-cue on a lookahead change.

## 1. Scope

### 1.1 The editor job

The editor from AU1 has balanced dialogue, music, and effects tracks. They now need to roll low
end off a boomy lav, tame a sibilant band, compress the dialogue bus so it sits, gate a noisy
room between lines, and put a true-peak limiter on the master so nothing clips on delivery.
They need to hear every change while playing, see gain reduction, and have the agent able to do
and measure the same, with spectrum evidence to justify an EQ move.

### 1.2 The two parts

**Part A — EQ and dynamics nodes.** Three new effect descriptors (`audio_parametric_eq`,
`audio_gate`, `audio_true_peak_limiter`), four neutral-preserving additions to
`audio_compressor`, `bypass` on all eight audio descriptors; the biquad, dynamics, and
true-peak DSP; document-derived processing latency with full compensation and per-family stem
trim; per-node gain-reduction telemetry; the `upsert_audio_bus` prose rewrite and
`audio_parametric_eq_pattern_documentation()`. **No new operation, no new document field, no
UI.**

**Part B — Bus and master control.** `AudioBus.gain_tenth_db`; `AudioMix.master: AudioMaster`
with `SetAudioMaster`; `AudioMix.pan_law: PanLaw` with `SetPanLaw`; `AudioMix::next_bus_id`;
the widened seek-preroll predicate; the live structural diff; the in-dock chain pane, bus and
master faders, and "+ Bus"; `Analysis::mix_spectrum` and `get_audio_spectrum`.

**How the roadmap row is discharged (A12, N5/Q4).** The AU2 row's **exit gate**
(ROADMAP-AND-WORKFLOWS.md:646) has three clauses:

1. *"Filter magnitude response matches the analytic transfer function at pinned frequencies"* —
   closed by **Part A** (A5).
2. *"gain reduction and ceiling are measured on synthetic material"* — closed by **Part A**
   (A11, A12).
3. *"playback/export parity through every node"* — **Part A closes the bus half** (A17: a bus
   chain carrying all five node kinds); **Part B closes it** by extending the same fixture with
   a master chain (B10). Part A's `parity_document_with_full_chain` has no master chain by
   construction, because Part A adds no master.

Part B additionally discharges AU1's standing clause *"serialized defaults are omitted and
hand-edited values are rejected on load"* (B2, B4) and the row's remaining **deliverables**
(the Deliverable column, not the exit gate): *"bus and master control editing in the mixer;
constant-power pan as a second law; spectrum evidence for the agent"*. The roadmap status line
under the staged table says exactly this: Part A leaves clause 3 half-open and Part B closes it.

### 1.3 Out of scope (named deferrals)

- Bus mute/solo and bus pan.
- Drag handles on the EQ curve; the well is read-only in AU2.
- Parameter smoothing for non-gain live changes (§6.10).
- Per-track pan-law override; mid/side or multiband processing.
- **BS.1770-4 true-peak conformance** of the delivered file, which needs the ITU coefficient
  table (OPEN-4, AU3).
- Moving `plan_audio_normalization` to the true-peak limiter, and any loudness targeting (AU3).
  The planner keeps emitting `audio_compressor` + `audio_gain` + the legacy `audio_limiter`
  (§4.2).
- Automation editing UI for bus and master parameters (AU4; the data model already carries the
  curves).
- A spectrum display in the UI (AU3 metering surface).
- A processor checkpoint cache for seek preroll; the M33 limit stands.
- A document-wide effect-id helper. Effect ids are unique **per owner** (N1): the editor
  allocates `max(chain effect ids) + 1` per chain, exactly as the inspector allocates per clip
  (`next_effect_id(clip)`, inspector_ui.rs:302-311). `color_status::next_effect_id`
  (color_status.rs:1642-1653) and its four call sites are untouched.
- Annotating `remove_audio_bus` destructive (facts-agent §8 records the inconsistency with
  `plan_preview`); AU2 does not touch it.

### 1.4 Documentation

Part A: this contract; `CHANGELOG.md` `### Added`/`### Changed`; `MEDIA-POLICY.md` "Playback
audio mixdown" (:106-126) — the stage-order sentence and one new latency paragraph (§3.6);
**`docs/M33-PARAMETRIC-DEPTH-VERIFICATION.md`** — its bus-effect list (:46-51) gains the three
new names **and the omitted `audio_limiter`**, and its one-pole-EQ limit (:110-113) gains an AU2
pointer; `M36-AGENT-RUNTIME-EFFICIENCY.md` (:107-108) gains a Part A row.

Part B: `README.md` audio bullet (line 36); `CHANGELOG.md`; `ROADMAP-AND-WORKFLOWS.md` — the
AU2 row text is unchanged, and the status paragraph under the staged table names the two parts
and the clause-3 split of §1.2; `DESIGN.md` Mixer section (:326-352, §6.9); `M36` gains a Part B
row (129 tools).

---

# Part A — EQ and dynamics nodes

---

## 2. Part A core model

### 2.1 Descriptor table (R4, A8)

`EFFECT_DESCRIPTORS` (effect.rs:1326) grows from 22 to 25 entries; the three new audio
descriptors are appended after `audio_limiter` (effect.rs:1751-1758). `is_audio_effect`
(effect.rs:2745-2750) extends its `matches!` to eight names. **Forty new
`EffectParameterDescriptor` rows** are added (8 `bypass` + 4 compressor + 18 EQ + 6 gate +
4 limiter) and **33 new `EffectUniform` variants** (effect.rs:5-73; today's audio variants are
:51-64), because the eight `bypass` rows share one variant (A8). All 33 join the compositor's
exhaustive ignore arm (compositor.rs:2658-2672), which today ends
`| EffectUniform::DuckRelease | EffectUniform::ColorNode => {}`.

**One shared bypass descriptor (A8).** Following `COLOR_NODE_BYPASS_DESCRIPTOR`
(effect.rs:238-244) — one shared `EffectParameterDescriptor` const with one uniform, reused by
every colour node (effect.rs:212, :229, :260, :440, :963) — AU2 declares

```rust
/// AU2 §2.1: the shared bypass control of every audio node.
const AUDIO_BYPASS_DESCRIPTOR: EffectParameterDescriptor = EffectParameterDescriptor {
    name: "bypass",
    min: 0,
    max: 1,
    neutral: 0,
    uniform: EffectUniform::AudioBypass,
};
```

reused by all eight audio descriptors — `audio_gain`, `audio_eq`, `audio_compressor`,
`audio_ducking`, `audio_limiter`, `audio_parametric_eq`, `audio_gate`,
`audio_true_peak_limiter`. `bypass = 1` turns the gain computer off; **the node's delay line is
still applied** (R5b), so bypass never changes `L`. Hold-only (§2.2). Nothing requires uniform
variants to be unique per descriptor: the compositor routes by variant and every audio variant
lands in the ignore arm.

**Note on neutrality.** "Every audio descriptor's neutral is an identity" is *already false*:
`audio_limiter`'s `ceiling_tenth_db` neutral is **-10**, not 0 (effect.rs:1751-1758), and
`audio_ducking`'s neutrals are a **working duck** (`threshold_tenth_db = -300`,
`reduction_tenth_db = 120`, effect.rs:1719-1750). AU2 does not change either, and
`audio_true_peak_limiter` inherits a weaker claim (§6.8, §6.10).

**`audio_compressor` — four additions.** The existing five parameters (effect.rs:1678-1715) are
unchanged, so every existing document renders as before.

| name | min | max | neutral | unit | uniform | semantics |
| --- | --- | --- | --- | --- | --- | --- |
| `knee_tenth_db` | 0 | 240 | 0 | tenth dB | `CompressorKnee` | total soft-knee width `W`, centred on the threshold. **0 takes the AU1 branch verbatim** (R24, §3.3) |
| `detector` | 0 | 1 | 0 | flag | `CompressorDetector` | 0 = instantaneous peak (today), 1 = RMS over `rms_window_milliseconds`. Hold-only |
| `rms_window_milliseconds` | 1 | 100 | 10 | ms | `CompressorRmsWindow` | boxcar RMS window; ignored when `detector` is 0. Minimum is 1, never 0 |
| `lookahead_milliseconds` | 0 | 10 | 0 | ms | `CompressorLookahead` | signal-path delay inside the node; the detector reads the undelayed input. **Never keyed**, read once at construction (§2.2) |

**`audio_parametric_eq` — 19 parameters** (18 rows plus the shared `bypass`). All-neutral is an
exact identity: every gain 0, the high-pass off, the output trim 0.

| name | min | max | neutral | unit | uniform | semantics |
| --- | --- | --- | --- | --- | --- | --- |
| `bypass` | 0 | 1 | 0 | flag | `AudioBypass` (shared) | as above |
| `high_pass_hertz` | 0 | 1000 | 0 | Hz | `ParametricEqHighPassHertz` | 0 disables the section; otherwise a 2nd-order Butterworth high-pass at this corner, `Q = 1/sqrt(2)` |
| `low_shelf_hertz` | 20 | 1000 | 100 | Hz | `ParametricEqLowShelfHertz` | shelf corner |
| `low_shelf_gain_tenth_db` | -240 | 240 | 0 | tenth dB | `ParametricEqLowShelfGain` | shelf gain, `S = 1` |
| `band1_hertz` | 20 | 20000 | 120 | Hz | `ParametricEqBand1Hertz` | peaking band 1 centre |
| `band1_gain_tenth_db` | -240 | 240 | 0 | tenth dB | `ParametricEqBand1Gain` | peaking band 1 gain |
| `band1_q_hundredths` | 10 | 1800 | 71 | hundredths | `ParametricEqBand1Q` | peaking band 1 Q, 0.10 … 18.00 |
| `band2_hertz` | 20 | 20000 | 500 | Hz | `ParametricEqBand2Hertz` | peaking band 2 centre |
| `band2_gain_tenth_db` | -240 | 240 | 0 | tenth dB | `ParametricEqBand2Gain` | |
| `band2_q_hundredths` | 10 | 1800 | 71 | hundredths | `ParametricEqBand2Q` | |
| `band3_hertz` | 20 | 20000 | 2000 | Hz | `ParametricEqBand3Hertz` | peaking band 3 centre |
| `band3_gain_tenth_db` | -240 | 240 | 0 | tenth dB | `ParametricEqBand3Gain` | |
| `band3_q_hundredths` | 10 | 1800 | 71 | hundredths | `ParametricEqBand3Q` | |
| `band4_hertz` | 20 | 20000 | 8000 | Hz | `ParametricEqBand4Hertz` | peaking band 4 centre |
| `band4_gain_tenth_db` | -240 | 240 | 0 | tenth dB | `ParametricEqBand4Gain` | |
| `band4_q_hundredths` | 10 | 1800 | 71 | hundredths | `ParametricEqBand4Q` | |
| `high_shelf_hertz` | 1000 | 20000 | 8000 | Hz | `ParametricEqHighShelfHertz` | shelf corner |
| `high_shelf_gain_tenth_db` | -240 | 240 | 0 | tenth dB | `ParametricEqHighShelfGain` | shelf gain, `S = 1` |
| `output_gain_tenth_db` | -240 | 240 | 0 | tenth dB | `ParametricEqOutputGain` | post-cascade trim |

**`audio_gate` — 7 parameters.** All-neutral is an exact identity twice over: `ratio_hundredths`
100 is 1:1 expansion and `range_tenth_db` 0 is no attenuation.

| name | min | max | neutral | unit | uniform | semantics |
| --- | --- | --- | --- | --- | --- | --- |
| `bypass` | 0 | 1 | 0 | flag | `AudioBypass` (shared) | as above |
| `threshold_tenth_db` | -600 | 0 | -600 | tenth dB | `GateThreshold` | expansion begins below this level |
| `ratio_hundredths` | 100 | 2000 | 100 | hundredths | `GateRatio` | downward expansion ratio; 100 is 1:1 |
| `range_tenth_db` | 0 | 800 | 0 | tenth dB | `GateRange` | maximum attenuation, as a positive figure |
| `attack_milliseconds` | 1 | 1000 | 1 | ms | `GateAttack` | time constant while the gate opens |
| `hold_milliseconds` | 0 | 1000 | 10 | ms | `GateHold` | the gate stays open this long after the level falls below threshold |
| `release_milliseconds` | 10 | 5000 | 100 | ms | `GateRelease` | time constant while the gate closes |

**`audio_true_peak_limiter` — 5 parameters.**

| name | min | max | neutral | unit | uniform | semantics |
| --- | --- | --- | --- | --- | --- | --- |
| `bypass` | 0 | 1 | 0 | flag | `AudioBypass` (shared) | as above |
| `ceiling_tenth_db` | -120 | 0 | 0 | tenth dB | `TruePeakCeiling` | output ceiling in dBFS |
| `lookahead_milliseconds` | 1 | 10 | 5 | ms | `TruePeakLookahead` | signal-path delay and gain-window length. **Never keyed** |
| `release_milliseconds` | 1 | 1000 | 50 | ms | `TruePeakRelease` | exponential release of the gain |
| `true_peak` | 0 | 1 | 1 | flag | `TruePeakDetector` | 1 = a 4x oversampled inter-sample peak detector (§3.5), 0 = sample peak. Hold-only |

`audio_eq` (fixed 200 Hz / 4 kHz one-pole crossovers) and `audio_limiter` (the stateless clamp)
are **retained, valid, and processed** unchanged apart from the shared `bypass`. They are not
offered in the human "+ Effect" menu (§6.8) and the agent prose steers away from them (§4.1).

**Runtime neutral duplication.** `audio.rs` re-hardcodes neutrals as `audio_value` fallbacks
(audio.rs:475-476, :492, :509-513, fn :996-998). Every new parameter's runtime fallback equals
its descriptor neutral, pinned by a media test that walks the eight audio entries of
`EFFECT_DESCRIPTORS` and asserts a chain built from an `Effect` with an empty `parameters` map
is bit-identical to one carrying every parameter at its descriptor neutral.

### 2.2 Hold-only and non-automatable parameters (R17, R5a)

Two rules, both enforced in `validate_audio_bus` (operation.rs:3945-4018) and, in Part B, in
`validate_audio_master`:

1. **Hold-only.** `is_hold_only_parameter` (operation.rs:3368-3385) gains an **audio branch**
   before its `ColorNodeKind::from_effect_name` early return: for an `is_audio_effect` name it
   returns true for `bypass`, `detector`, and `true_peak`. A curve on one of those with any
   interpolation other than `KeyframeInterpolation::Hold` is rejected with the existing
   `OpError::NonHoldKeyframeParameter { effect, name }` (operation.rs:726). The clip path
   already reaches this check inside `validate_curve` (check at operation.rs:3327, error at
   :3330); the bus path does not call `validate_curve` — it is clip-scoped, taking
   `clip: ClipId` and `clip_duration` (operation.rs:3306-3314) — so `validate_audio_bus` calls
   `is_hold_only_parameter` directly in its keyframe loop. Frequency, Q, gain, and time
   parameters keep every interpolation; linear-in-hertz is musically wrong but is a documented
   consequence of "integer document controls with stable units" (ROADMAP:630-633) and is
   recorded in §6.10.
2. **Latency-bearing parameters are never keyed.** `lookahead_milliseconds` on
   `audio_compressor` and on `audio_true_peak_limiter` rejects **any** `keyframes` entry, with
   the existing `OpError::InvalidEffectAutomation { effect, name, reason }` shape
   (operation.rs:801-806, declared at :802) and
   `reason = "sets processing latency and cannot be keyframed"` — no new variant (R5).

```rust
/// AU2 §2.2: parameters read once when a chain runtime is built, never per frame.
#[must_use] pub fn is_static_audio_parameter(effect: &str, parameter: &str) -> bool;

impl Effect {
    /// AU2 §2.2: the stored static value of one parameter, ignoring keyframes.
    /// `None` when the parameter is absent; callers fall back to the descriptor
    /// neutral. Distinct from `integer_parameter_at` (model.rs:212-222), which
    /// resolves a curve.
    #[must_use] pub fn static_integer_parameter(&self, name: &str) -> Option<i64>;
}
```

`is_static_audio_parameter` is true exactly for
`("audio_compressor", "lookahead_milliseconds")` and
`("audio_true_peak_limiter", "lookahead_milliseconds")`. `AudioEffectRuntime::new`
(audio.rs:403-416) reads the value once with `Effect::static_integer_parameter`;
`process_frame` never reads it.

### 2.3 Validation and the per-chain budget

```rust
// core, beside the audio model
/// AU2 §3.6: the per-chain lookahead budget, in milliseconds (R18).
pub const CHAIN_LOOKAHEAD_MILLISECONDS: i64 = 20;

/// AU2 §3.6: the processing latency one document's chains declare.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct ChainLookahead { pub bus_stage: i64, pub master_stage: i64 }

impl AudioMix {
    /// AU2 §3.6: `bus_stage` = max over buses of that bus's declared lookahead
    /// sum; `master_stage` = the master chain's sum. Zero for every pre-AU2
    /// document.
    #[must_use] pub fn lookahead_milliseconds(&self) -> ChainLookahead;
}
/// The declared processing latency of one ordered chain, in milliseconds.
#[must_use] pub fn chain_lookahead_milliseconds(effects: &[Effect]) -> i64;
```

`chain_lookahead_milliseconds` sums the **static** `lookahead_milliseconds` of every
`audio_compressor` and `audio_true_peak_limiter` in the slice, using the descriptor neutral when
the parameter is absent, **whether or not the node is bypassed** (R5b). In Part A
`master_stage` is always 0 (there is no master chain yet); Part B fills it. `ChainLookahead`
deliberately carries no `total()`: latency is only ever converted to sample frames through
`graph_latency_frames` (§3.6), never by summing milliseconds first (A3).

New `OpError` variant for Part A, declared beside the existing audio errors
(operation.rs:464-494):

| variant | message |
| --- | --- |
| `AudioBusLookaheadExceeded { bus, milliseconds }` | `audio bus {bus} declares {milliseconds} ms of lookahead, beyond the 20 ms chain budget` |

`validate_audio_bus` gains, after its existing effect loop, the budget check
(`chain_lookahead_milliseconds(&bus.effects) > CHAIN_LOOKAHEAD_MILLISECONDS`), and inside the
keyframe loop the two rules of §2.2. Because `validate_audio_mix` (operation.rs:3924-3943) is
reached both from the operation (operation.rs:1408) and from the document invariant
(`validate_document`, operation.rs:3897), one rule covers the operation, `ApplyOp::apply`'s
before/after clone check (operation.rs:409-419), `Document::validate` (model.rs:936-942), the
app's `load_document` (app.rs:1864-1869), and `Core::spawn` (actor.rs:147). `OpError` keeps its
`Debug, Clone, PartialEq, Eq, Error` derives (operation.rs:423) and stays non-`non_exhaustive`.

At 20 ms a chain can carry a 10 ms lookahead compressor **and** a 10 ms true-peak limiter, which
is the case R18 was raised to permit.

### 2.4 Part A contract tests

- `tests/contracts.rs:2519-2529` descriptor classification sweep and
  `compositor.rs:5399-5426` (`legacy_shader_routing_agrees_with_core_effect_classification`)
  are **unchanged and must still pass**: the three new descriptors stay unclassified for colour
  compatibility staging, which holds only if their names are absent from
  `LEGACY_DISPLAY_EFFECT_NAMES` and `POST_PRIMARY_LUT_EFFECT_NAMES`. Neither test knows about
  uniforms (A7).
- `tests/contracts.rs:3009-3092`
  (`audio_buses_validate_routing_effect_domains_and_project_keyframes_atomically`) gains three
  rejected buses — a chain declaring 21 ms of lookahead, a `Linear` curve on `bypass`, and any
  curve on `lookahead_milliseconds` — each asserting `doc == before`.
- New `crates/kinewright-core/tests/au2_core.rs`, mirroring `tests/au1_core.rs`: one test per §7
  Part A item, each with a `/// AU2 §7 item An.` doc comment.

## 3. Part A media DSP

### 3.1 Stage order (normative, Part A)

```
clip shaping (gain, fades, transition ramps)          — unchanged, before mix_chunk
→ track stage: gate → gain → balance pan               — unchanged (AU1 §3.1)
→ routing: sidechain taps read the POST-track-stage, PRE-delay signal
→ per bus, in document order:
     bus effects in `bus.effects` order (a bypassed node still applies its delay)
   → bus alignment pad to L_bus
→ unrouted path: plain delay to L_bus
→ master sum
→ master limiter clamp +/-1.0 (unchanged, applied by callers) → master meter (unchanged)
```

Part B inserts the bus fader, the master fader, and the master chain (§5.6). The final clamp
(`limit_audio_mix`, audio.rs:144-148, called from audio.rs:1275-1279 and export.rs:908) is
unchanged and is still the only clamp in the path.

### 3.2 Biquads

One generic transposed-direct-form-II section, state in `f64` per channel, sample in and out
`f32`:

```
y = b0*x + s1;  s1 = b1*x - a1*y + s2;  s2 = b2*x - a2*y
```

Coefficients are RBJ cookbook forms, computed in `f64` from the runtime `sample_rate` and
divided through by `a0`. With `f0` the section frequency, `fs` the rate, `dB` the section gain:

```
w0 = 2*PI*f0/fs      cw = cos(w0)      A = 10^(dB/40)
```

**High-pass** (2nd-order Butterworth, `Q = 1/sqrt(2)`, `alpha = sin(w0)/(2*Q)`):

```
b0 = (1+cw)/2      b1 = -(1+cw)      b2 = (1+cw)/2
a0 = 1 + alpha     a1 = -2*cw        a2 = 1 - alpha
```

**Low shelf** (`S = 1`, so `alpha = sin(w0)/2 * sqrt(2)`):

```
b0 =    A*((A+1) - (A-1)*cw + 2*sqrt(A)*alpha)
b1 =  2*A*((A-1) - (A+1)*cw)
b2 =    A*((A+1) - (A-1)*cw - 2*sqrt(A)*alpha)
a0 =      (A+1) + (A-1)*cw + 2*sqrt(A)*alpha
a1 =   -2*((A-1) + (A+1)*cw)
a2 =      (A+1) + (A-1)*cw - 2*sqrt(A)*alpha
```

**High shelf** (`S = 1`, same `alpha`):

```
b0 =    A*((A+1) + (A-1)*cw + 2*sqrt(A)*alpha)
b1 = -2*A*((A-1) + (A+1)*cw)
b2 =    A*((A+1) + (A-1)*cw - 2*sqrt(A)*alpha)
a0 =      (A+1) - (A-1)*cw + 2*sqrt(A)*alpha
a1 =    2*((A-1) - (A+1)*cw)
a2 =      (A+1) - (A-1)*cw - 2*sqrt(A)*alpha
```

**Peaking EQ** (`alpha = sin(w0)/(2*Q)`, `Q = q_hundredths/100`):

```
b0 = 1 + alpha*A   b1 = -2*cw        b2 = 1 - alpha*A
a0 = 1 + alpha/A   a1 = -2*cw        a2 = 1 - alpha/A
```

Rules:

- **Frequency clamp.** Every `f0` is clamped to `20.0 ..= 0.45 * fs` before `w0` is formed, so a
  20 kHz band on a 44.1 kHz device becomes 19 845 Hz rather than an unstable section.
- **Small-value squelch (A25).** After each sample frame, a state word whose magnitude is below
  `1e-30` is set to `0.0` — both `s1` and `s2`, per channel, per section. This is **not** a
  denormal flush: state is `f64`, whose subnormal threshold is about `2.2e-308`, so true
  denormals are unreachable in practice. It is a cheap squelch that keeps a settled section from
  running its state down toward subnormals, and its cost is that a decay tail below about
  -600 dBFS is truncated, so a filter's tail is not bit-reversible.
- **Section order inside the node:** high-pass → low shelf → band 1 → band 2 → band 3 → band 4 →
  high shelf → `output_gain_tenth_db`.
- **Every section runs whenever the node is active (A26).** A peaking or shelf section at
  `A == 1` is already an exact identity in steady state (`b == a` after dividing by `a0`), so
  skipping it buys one branch and costs a discontinuity every time an automated band gain passes
  through 0, because the section's stored tail would be thrown away and re-entered from zero.
  The **only** skip is the whole-node identity: when every gain is 0 and `high_pass_hertz == 0`
  the node is a bit-exact pass-through, and that is tested **at node level** (A6), not per
  section. `high_pass_hertz == 0` disables the high-pass section, which has no gain parameter
  and therefore no identity setting.
- **Per-project-frame evaluation.** Parameters are read once per project frame, not per sample
  frame. `audio_value` already returns a project-frame staircase (automation.rs:78-120; 4 800
  sample frames per step at 48 kHz / 10 fps), so the output is identical to per-sample-frame
  evaluation (pinned by A19). The node caches the last parameter tuple and recomputes
  coefficients only when it changes, replacing 19 `BTreeMap` lookups per sample frame with 19
  per project frame. **The cache key is `(project_at, parameter epoch)`, not `project_at`
  alone** (A13): a live retarget bumps the runtime's epoch, so new values take effect on the
  next sample frame rather than at the next project-frame boundary — which at 10 fps would be
  100 ms and on a 1 fps timelapse a whole second (§5.8).

### 3.3 Compressor

Detector, per sample frame, on the **undelayed** input:

- `detector == 0`: `level = max over the frame's channels of |sample|` — today's behaviour
  (audio.rs:479-481).
- `detector == 1`: a boxcar running mean of the per-frame mean square across channels over
  `stage_latency_frames(rms_window_milliseconds, fs).max(1)` sample frames; `level = sqrt(mean)`.
  A circular `Vec<f64>` with a running sum, allocated at construction, resized only on a
  structural rebuild.

`L = amplitude_db(level)` (audio.rs:1005-1008), `T = threshold_tenth_db/10`,
`R = ratio_hundredths/100`, `W = knee_tenth_db/10`.

**`W == 0` takes the AU1 branch verbatim (R24).** The gain computer's first statement is an
early return that evaluates the existing expression at audio.rs:484-490 unchanged:

```
if W == 0 {
    target = if level_db > threshold_db && ratio > 1.0 {
        10.0_f32.powf(((threshold_db + (level_db - threshold_db) / ratio) - level_db) / 20.0)
    } else { 1.0 }
}
```

so an existing-shaped document is bit-identical, not merely close. The knee interpolation is
reached only for `W > 0` (it divides by `W`):

```
if 2*(L-T) < -W      : y = L
if |2*(L-T)| <= W    : y = L + (1/R - 1) * (L - T + W/2)^2 / (2*W)
if 2*(L-T) > W       : y = T + (L - T)/R
target = 10^((y - L)/20)
```

At the knee midpoint `L == T` the reduction is `(1 - 1/R) * W/8` dB.

Attack and release reuse `smooth_gain` (audio.rs:1015-1032), unchanged in behaviour; its two
time arguments are **renamed** `falling_milliseconds` and `rising_milliseconds`, since the
function already selects the first when `target < *current`. The compressor passes
`(attack_milliseconds, release_milliseconds)`.

**Lookahead (N4).** The node's delay line is `stage_latency_frames(lookahead_milliseconds, fs)`
sample frames per channel — the **same truncating conversion §3.6's pad arithmetic uses**, never
`round`. No line is allocated at 0, which is also what the parameter's neutral gives. The signal
path is delayed; the detector reads the undelayed input. Makeup (`makeup_gain_tenth_db`)
multiplies after the envelope, unsmoothed, as today. The envelope stays stereo-linked, one per
node. Gain reduction reported to §3.8: `min(1.0, envelope)` over the chunk.

### 3.4 Gate / expander

Detector: peak across the frame's channels, as the compressor's peak mode.
`L = amplitude_db(level)`, `T = threshold_tenth_db/10`, `R = ratio_hundredths/100`,
`Rg = range_tenth_db/10`.

```
hold_frames = stage_latency_frames(hold_milliseconds, fs)
if L >= T                 : hold_counter = hold_frames; target_db = 0
else if hold_counter > 0  : hold_counter -= 1;          target_db = 0
else                      : target_db = max(-Rg, (R - 1) * (L - T))
target = 10^(target_db/20)
```

`(L - T)` is negative below the threshold, so `(R - 1)*(L - T)` is a downward expansion that
reaches the floor `-Rg` and stops. `R == 1` gives `target_db == 0` at every level, and `Rg == 0`
gives `target_db == 0` at every level: either alone makes a neutral gate an exact identity.

The envelope uses `smooth_gain` with the arguments transposed —
`smooth_gain(envelope, target, release_milliseconds, attack_milliseconds, fs)` — because the
first time argument is the *falling*-gain constant and a gate's falling gain is its release. One
envelope per node, stereo-linked. The gate declares zero lookahead. Gain reduction reported to
§3.8: `min(1.0, envelope)` over the chunk.

### 3.5 True-peak limiter (A1, N3, N4, N5/Q1, A27)

**Peak estimate.** With `true_peak == 0` the estimate is `max |sample|` across the frame's
channels. With `true_peak == 1` the node runs a 4x polyphase windowed-sinc interpolator per
channel: 48 taps, 12 per phase. This has the **structure** of ITU-R BS.1770-4 Annex 2 (48 taps,
four phases of twelve) with **this contract's own coefficients**; it is not the ITU coefficient
table and must not be described as one (OPEN-4).

```
for n in 0..48:
    x_n  = (n - 23.5) / 4                     // never zero, so sinc needs no special case
    w_n  = 0.42 - 0.5*cos(2*PI*(n+0.5)/48) + 0.08*cos(4*PI*(n+0.5)/48)   // Blackman
    h[n] = sin(PI*x_n)/(PI*x_n) * w_n
then each phase p in 0..4 is normalised so that sum over k in 0..12 of h[4k+p] == 1
```

The node keeps a 12-sample-frame history per channel. At emission index `i`, phase `p` produces
`y_p = sum over k in 0..12 of h[4k+p] * x[i-k]`; the raw estimate is the maximum of `|y_p|` over
the four phases and every channel. Taps are computed once into a `OnceLock`.

**Detector group delay.** A linear-phase FIR of length 48 at 4x has a group delay of
`(48-1)/2 = 23.5` output samples = 5.875 input sample frames, so the estimate produced at
emission index `i` describes the continuous signal around frame `i - 5.875`. The contract names

```rust
/// AU2 §3.5: the true-peak detector's group delay in input sample frames,
/// `round((TAPS - 1) / 2 / OVERSAMPLE)` = round(5.875).
const TRUE_PEAK_GROUP_DELAY_FRAMES: usize = 6;
```

and **stores the estimate against frame `i - D`**, where `D = TRUE_PEAK_GROUP_DELAY_FRAMES` when
`true_peak == 1` and `D = 0` when `true_peak == 0`. `D` is absorbed **inside** the node's
lookahead: the node's total signal delay stays exactly
`Lh = stage_latency_frames(lookahead_milliseconds, fs)` (N4, N5/Q1), so §3.6's identity
`pad = stage_latency_frames(L_bus) - Σ stage_latency_frames(node)` is untouched and
`chain_lookahead_milliseconds` neither over- nor under-declares.

**Gain computer.** With `C = 10^(ceiling_tenth_db/200)`, `Lh` as above, and the window
`Wn = Lh - D + 1`:

```
required[k] = min(1.0, C / max(peak_estimate[k], 1e-9))      // indexed by the frame it describes
m[i]        = min over k in [i-Lh, i-D] of required[k]       // Wn frames wide, causal
r[i]        = if m[i] >= 1.0 && r[i-1] >= 1.0 { 1.0 }        // explicit identity short-circuit
              else { min(m[i], c*r[i-1] + (1-c)*m[i]) }      // c = exp(-1/(release_ms*fs/1000))
g[i]        = mean of r[i-Wn+2 ..= i]                        // boxcar of length max(Lh - D, 1)
out[i]      = x[i-Lh] * g[i]
```

Everything is indexed in **emission time** `i` (N3): `m` is a trailing window, `g` a trailing
boxcar, and the sample leaving the node at index `i` is the one that entered `Lh` frames ago.

**Ceiling proof.** Fix an emission index `i`; the emitted sample is `d = i - Lh`. Every boxcar
term `j` lies in `[i - Lh + D + 1, i]`. For each such `j`, `d ∈ [j - Lh, j - D]`: the lower bound
because `j <= i` gives `d = i - Lh >= j - Lh`, and the upper bound because
`j >= i - Lh + D + 1 > i - Lh + D` gives `j - D > i - Lh = d`. So `required[d]` is inside the
window that defines `m[j]`, hence `m[j] <= required[d]`; `r[j] <= m[j]` by construction; and
`g[i]` is a convex combination of those `r[j]`, so `g[i] <= required[d]` and the emitted sample
`x[d] * g[i]` never exceeds the ceiling — for any release value, at any sample rate, with either
detector. A step in `required` at frame `s` enters `m` at emission index `s + D` and `g` is fully
down by `s + Lh - 1`, before sample `s` is emitted at `s + Lh`: the linear attack across the
lookahead holds, and the boxcar of a step is a ramp of length `Lh - D`.

**Requirement and fallback.** The construction needs `Lh >= D + 1`, i.e. `Lh >= 7` frames with
the true-peak detector — satisfied by every real device rate (`Lh = 48` at the 1 ms minimum at
48 kHz; the threshold is about 7 kHz). When `Lh < D + 1` the node **falls back to sample-peak
detection with `D = 0`**, which needs only `Lh >= 1`; when `Lh == 0` (N4: only at the low test
rates, e.g. rate 1 000 in `limiter_enforces_its_declared_sample_ceiling`, audio.rs:1881-1906)
the node applies no delay, the window is one frame, and the boxcar length is `max(Lh - D, 1) = 1`.
These degenerate cases are recorded in §6.10.

**Under-ceiling identity (A27).** The short-circuit in `r` is structural, not incidental:
without it, `r = min(m, c*r_prev + (1-c)*m)` at `m = r_prev = 1` evaluates `c + fl(1 - c)`, which
is exactly `1.0` only because `c >= 0.5` makes `1 - c` exact by Sterbenz's lemma — and `c` drops
below 0.5 at the low test rates this codebase uses (at rate 1 000 with a 1 ms release,
`c = exp(-1) = 0.368`). With the short-circuit, a signal whose estimate never exceeds the ceiling
has `required == m == r == g == 1.0` and `out[i] == x[i-Lh]` **bit-exactly at every rate** — an
exact identity, up to the node's constant delay.

**Cost (R21).** The sliding minimum is a **monotonic deque** over the `Wn`-frame window: each
frame pushes once and pops amortised once, so the running minimum is amortised O(1) per sample
frame rather than the O(Lh) rescan D4's prose implies (480 frames × 48 000 × 2 channels ≈ 46 M
comparisons per second on the worker thread that also answers `Control::Thumbnail`
synchronously). The boxcar likewise keeps a running sum. A19 records the per-second cost of a
worst-case chain so `LIVE_FILL_MILLISECONDS` (audio.rs:27-30) does not have to be revisited.

`audio_limiter`, the stateless clamp (audio.rs:508-518), is unchanged. Gain reduction reported to
§3.8: `min` of `g` over the chunk.

### 3.6 Document-derived latency (R1, A3, A18)

D3's mandatory constant latency is **withdrawn**. Latency is a function of the document,
computed once when the processor is built and held constant for that processor's life:

```
L_bus    = max over buses of chain_lookahead_milliseconds(bus.effects)   // 0 today
L_master = chain_lookahead_milliseconds(master.effects)                  // 0 today; Part B
```

Every bus chain **and the unrouted path** are padded to `L_bus`; the master chain is padded to
`L_master`. In sample frames:

```rust
/// AU2 §3.6: one stage's latency in sample frames at one rate. Truncating.
fn stage_latency_frames(milliseconds: i64, sample_rate: u32) -> usize;   // rate * ms / 1000

/// AU2 §3.6: the processor's total input-to-output latency in sample frames.
/// The SUM OF THE TWO TRUNCATIONS, never a truncation of the sum (A3).
pub(crate) fn graph_latency_frames(lookahead: &ChainLookahead, sample_rate: u32) -> usize {
    stage_latency_frames(lookahead.bus_stage, sample_rate)
        + stage_latency_frames(lookahead.master_stage, sample_rate)
}
```

`graph_latency_frames` is the **only** function any compensation site uses (§3.7, §3.9c). Using
`stage_latency_frames(L_bus + L_master, fs)` instead would be off by one sample frame wherever
the rate is not a multiple of 1 000: at `fs = 44 100` with `L_bus = L_master = 5`, the actual
latency is `220 + 220 = 440` while `stage_latency_frames(10, 44 100) = 441`. Export is always
`AUDIO_RATE = 48 000` (export.rs:27), where `48 * ms` is integer, but **playback runs at the
device rate** (`AudioMixProcessor::new(document, output_rate, …)`, audio.rs:1194-1199), and
44.1 kHz is the commonest device rate there is. A19 pins the 44 100 Hz case.

Per chain, the alignment pad is computed **in sample frames**, not milliseconds:

```
node_frames(chain) = sum over nodes of stage_latency_frames(node lookahead, fs)
pad_frames(chain)  = stage_latency_frames(L_bus, fs) - node_frames(chain)
```

`pad_frames` is non-negative because the floor function is superadditive —
`Σ_k floor(fs·ms_k/1000) <= floor(fs·(Σ_k ms_k)/1000)` — and because **every node's delay is
exactly `stage_latency_frames(node lookahead, fs)`** (N4, §3.3, §3.5), using the same truncating
conversion. It is *not* a consequence of §2.3's 20 ms budget, which bounds the millisecond sum
and says nothing about truncations. **Degenerate case:** at a rate where `L_bus > 0` but
`stage_latency_frames(L_bus, fs) == 0` (test rates below 1 kHz with a 1 ms chain), the whole bus
stage is a zero-length pad and no node applies any delay; §3.9(c)'s impulse pin degenerates
there and is asserted only at 48 000 and 44 100 Hz.

Every bus chain's input-to-output latency is therefore exactly `stage_latency_frames(L_bus, fs)`,
whatever the split, and inter-bus alignment is exact. A bypassed node keeps its delay line and
its length (§2.2), so toggling bypass changes no alignment.

**`L == 0` for every pre-AU2 document**, so export bytes, the parity harness, and all four
existing `mix_chunk` unit tests are untouched — including
`limiter_enforces_its_declared_sample_ceiling` (audio.rs:1881-1906), which mixes **two** sample
frames at rate 1 000, and `bus_gain_automation_uses_exact_project_frames` (audio.rs:1782-1821),
which mixes 11 frames at rate 10. No existing fixture is rewritten. `MEDIA-POLICY.md`'s new
paragraph reads:

> A chain that declares processing lookahead delays the whole mix by that much. The bus stage's
> latency is the largest lookahead any one bus declares, the master stage's is the master
> chain's own, and every other chain and the unrouted path are padded to match, so buses stay
> aligned with each other. The total is computed once when the processor is built and is zero
> for any document with no lookahead node, so every project written before AU2 mixes byte for
> byte as it did. Export mixes past the requested end by the total and drops the same number of
> leading sample frames; a playback seek runs past its target by the total and discards it, so
> output frame `k` still carries project frame `target + k` and the transport clock is
> unchanged. A live edit that changes any chain's lookahead stops and re-cues playback once.

### 3.7 Compensation and the per-family stem trim (R2, A2, A29)

Let `L = graph_latency_frames(&document.audio_mix.lookahead_milliseconds(), rate)`. Three wiring
sites, named:

1. **`AudioMixer::open`** sets `end_sample = frame_to_samples(project_end, output_rate,
   document.fps) + L` (**audio.rs:1203**), so `next_chunk_limited`'s
   `self.cursor_sample >= self.end_sample` early return (audio.rs:1237-1239) no longer truncates
   the tail: this is the end-of-programme flush, not a separate phase. Input frames at or past
   the project duration are fed zero-filled track buffers.
2. The discard loop bound `while mixer.cursor_sample < target_sample` (**audio.rs:1206**)
   becomes `target_sample + L` and runs **unconditionally**, including when
   `project_from == TimeCode::ZERO` — today the loop body never executes on a seek to zero.
   `needs_seek_preroll` (**audio.rs:1120-1122**) continues to govern only whether `decode_from`
   is moved back to zero; Part B widens its predicate (§5.6).
3. **`mix_pass`** raises `total_sample_frames` by `L` (**export.rs:801**), then, before the
   existing `keep_from` drain (**export.rs:908-923**), applies a **head and tail** trim per stem
   family (A2). Head counts, dropped **unconditionally** — unlike `keep_from`, which is inside
   `if keep_from > 0` (export.rs:913) and never runs for `mix_audio`, where `range.start == 0`:

   | family | tapped at | leading sample frames dropped |
   | --- | --- | --- |
   | track stems (`self.staged`, audio.rs:978-982) | pre-bus | 0 |
   | bus stems (audio.rs:970-974) | post-bus-chain | `stage_latency_frames(L_bus, AUDIO_RATE)` |
   | master | post-master-chain | `graph_latency_frames(&lookahead, AUDIO_RATE)` |

   Then **every family is truncated** to
   `(frame_to_samples(range.end) - frame_to_samples(range.start)) * channel_count` samples.
   Without the tail truncation the three families come out at three different lengths — track
   stems `total + L`, bus stems `total + L_master`, master `total` — and `measure_loudness`
   (export.rs:986, :1000, :1003) would report three different `sample_frames` for one requested
   range through `get_audio_levels` (server.rs:7186-7226), which is exactly the defect R7 was
   raised to kill. With head and tail applied, every stem covers exactly
   `[range.start, range.end)` in project sample frames and all three report the same
   `sample_frames`.

**Clock invariant (A29), normative.** Because `open` discards exactly `L` output frames and
`end_sample` is raised by exactly `L`, output frame *k* still carries project frame
`target + k`. `SharedClock::position` (engine.rs:77-92), the cpal callback's
`position.fetch_add` (audio.rs:1479-1496), the seek-target initialisation (audio.rs:1368-1370)
and the video presentation path (engine.rs:1547-1559) are therefore all untouched. This holds at
the edge too: a seek to the last frame with `L > 0` yields
`end_sample - (target_sample + L) = frame_to_samples(duration) - target_sample` output frames,
i.e. exactly one project frame, and `engine.rs:1484` already clamps `from` to `duration - 1`.

The two existing length assertions in the shared parity harness,
`assert_eq!(played.len(), exported.len())` (**audio.rs:2898**) and
`assert_eq!(seeked.len(), exported.len() - seek_sample)` (**audio.rs:2927**), are the regression
pins for the flush: they fail deterministically if `end_sample` is not extended. Under R1 this
whole section is dead code when `L == 0`, which is the point.

### 3.8 Gain-reduction telemetry (D9)

`MixMeters` (audio.rs:201-274) gains a gain-reduction table beside its track and bus tables:

```rust
/// One lock-free gain-reduction slot (AU2 §3.8). Telemetry, not state.
struct GainReductionState(AtomicU32);   // f32 bits, the MeterState discipline

pub(crate) struct MixMeters {
    tracks: Vec<(TrackId, MeterState)>,
    buses: Vec<(AudioBusId, MeterState)>,
    gain_reduction: Vec<(AudioChain, EffectId, GainReductionState)>,
    master: Arc<MeterState>,
}
impl MixMeters {
    /// AU2 §5.8: whether the installed table's key set still describes this
    /// document, so a live update can skip the rebuild (A33).
    pub(crate) fn matches_document(&self, document: &Document) -> bool;
}
```

```rust
// core media.rs, beside MixPeaks
/// Which processing chain a mix point belongs to (AU2 §3.8).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AudioChain { Bus(AudioBusId), Master }

#[derive(Debug, Clone, Default, PartialEq)]
pub struct MixPeaks {
    pub tracks: Vec<(TrackId, [f32; 2])>,
    pub buses: Vec<(AudioBusId, [f32; 2])>,
    pub master: [f32; 2],
    /// AU2 §3.8: per-node gain reduction in decibels, always >= 0.
    pub gain_reduction: Vec<(AudioChain, EffectId, f32)>,
}
```

`MixMeters::for_document` (audio.rs:210-226) builds the gain-reduction table by walking each bus
chain in document order — and, in Part B, the master chain — adding one slot for every effect
named `audio_compressor`, `audio_ducking`, `audio_gate`, or `audio_true_peak_limiter`. Nodes with
no gain computer get no slot. Each of those nodes records, once per chunk with overwrite
semantics, `(-20.0 * min_gain.log10()).max(0.0)` where `min_gain` is the minimum linear gain it
applied anywhere in the chunk. `MixMeters::clear` clears the new slots. Meters stay telemetry,
not state, and are not recorded during seek preroll. `AudioChain::Master` slots exist only from
Part B; Part A emits `Bus(..)` keys only. `MixChunkStems` (audio.rs:768-774) is unchanged: the
per-family trim lives entirely in `mix_pass`.

### 3.9 Part A evidence (D12 a–d)

**(a) Analytic filter pins (R11).** Measured through the real node with a 2 s tone at 48 kHz,
RMS over a whole number of periods across the last 0.5 s, compared in dB against literals
derived from the transfer function, **not** from the node's coefficients:

- **Peaking:** a 1 kHz band at `band1_gain_tenth_db = 60`, `Q = 1.00` reads exactly `+6.00 dB`
  at 1 000 Hz, ±0.01 (RBJ peaking gives `|H(f0)| = A^2 = 10^(G/20)`).
- **Shelves:** low shelf 250 Hz `+80` reads `+8.00 dB` at 20 Hz, `+4.00 dB` at 250 Hz, `0.00 dB`
  at 20 kHz; high shelf 4 kHz `-80` reads `-8.00 dB` at 20 kHz, `-4.00 dB` at 4 kHz, `0.00 dB`
  at 20 Hz. All ±0.02. (RBJ with `S = 1` gives `A = 10^(G/40)` at `f0`, i.e. exactly half the dB
  gain, and the full gain at DC or Nyquist.)
- **High-pass, at `fc = 1 000 Hz` and `fs = 48 000 Hz`:** `-3.0103 dB` at `fc`, ±0.01, and
  `-12.32 dB` at `fc/2` (500 Hz), ±0.05. The `fc` pin is **frequency-independent** —
  `H(e^{jw0}) = jQ` exactly for every `fc` and every rate — so it is additionally asserted at
  `fc = 100 Hz` and `fc = 8 000 Hz`. The `fc/2` figure is **not**: the digital response there is
  `-12.322 dB` at `fc = 1 kHz` and `-13.53 dB` at `fc = 8 kHz`, so no bare "`-12.3 dB` at
  `fc/2`" claim is made. The test comment records the warped closed form
  `r = tan(PI*f/fs)/tan(PI*fc/fs)`, `|H| = r^2/sqrt(1+r^4)`.
- A neutral `audio_parametric_eq` over a 1 000-frame pseudo-random buffer is bit-identical to its
  input (`==`, not a tolerance) — asserted **at node level** (A26) — as is a bypassed node of
  every kind against its delayed input.

**(b) Dynamics on synthetic material.**

- Below-threshold identity (`==`); 12 dB over at 4:1 settling 3 dB over within 1e-3; the knee
  midpoint at `(1 - 1/R) * W/8` dB (`T = -20, R = 4, W = 12` → 1.125 dB).
- **R24 pin:** one existing-shaped compressor document (`threshold_tenth_db = -120`,
  `ratio_hundredths = 400`, none of the four new parameters present) produces output
  bit-identical to a stored expectation.
- RMS versus peak on a 5 ms burst inside 100 ms of silence.
- Gate: `T = -30`, `R = 400`, `range = 240` on a `-60 dBFS` tone settles at exactly `-24 dB`,
  not `-120`; `R = 100` and `range = 0` are each independently an identity; the hold count is
  exact.
- **Limiter ceiling (R22, A1).** The output is verified with an **independent 16x windowed-sinc
  interpolator written in the test**, never with the node's own 4x estimator. Cases: an
  over-level tone with `true_peak` both 0 and 1, never exceeding the ceiling; **an isolated
  full-scale impulse** (not a tone) at `true_peak = 1` with `Lh` at its minimum for the rate,
  which is the case the detector's group delay would break if `D` were not indexed out; an
  under-ceiling signal passing bit-exactly after the `Lh`-frame delay, asserted at 48 000 Hz
  **and** at rate 1 000 with a 1 ms release (where `c < 0.5`, A27); and the release constant
  within 5 %. The `fs/4` case (a sine at 12 kHz sampled at 45°, continuous amplitude 1.0) pins
  **sample peak `-3.010 dBFS` ±0.01 and the node's detector `0 dBFS` ±0.3** — the contract's
  Blackman windowed-sinc recovers about `-0.17 dBFS`, and the ±0.3 tolerance is deliberate
  headroom over that margin (OPEN-4; the ITU coefficient table is AU3's).
- **R25 silent-arm pin:** a test iterates every name for which `is_audio_effect` returns true,
  builds a bus carrying that node at a non-neutral setting, and asserts the output differs from
  the input by more than 1e-3 on a full-scale tone. This is the one assertion that
  `process_frame`'s `_ => {}` arm (audio.rs:550) and `AudioEffectRuntime::new`'s second match
  (audio.rs:403-411) cannot pass by accident when their names disagree.

**(c) Latency.** With `L == 0` (any pre-AU2 document) `mix_chunk` output is bit-identical to
AU1's. With a 10 ms lookahead limiter on one bus: an impulse at input frame 0 emerges at output
frame `graph_latency_frames(&lookahead, rate)` and nowhere else, asserted at **48 000 Hz and
44 100 Hz** (A3); export puts an impulse at project frame `k` at exported frame `k`; the
programme tail survives the flush; `AudioMixer::open` at frame 5 emits its first output frame
from input frame 5, and so do `open` at frame 0 and `open` at the last frame (A29). **A2 pin:**
on a document whose bus carries a lookahead limiter, `measure_mix_levels` over a one-frame range
containing a single impulse reports that impulse in the track stem, the bus stem, and the
master, **and all three report the same `sample_frames`**. Tests that call `mix_chunk` directly
use a helper `fn latency_offset(document: &Document, rate: u32) -> usize` that forwards to
`graph_latency_frames`.

**(d) Parity through every node (R3).** `parity_document` (audio.rs:2809-2871) and
`parity_document_with_track_mix` (audio.rs:2875-2883) are **unchanged**, so the AU1 linear-ratio
assertion at audio.rs:2960-2963 is untouched. A new
`parity_document_with_full_chain(voice, bed, fps)` adds a parametric EQ, a gate, an RMS/lookahead
compressor, and a ducking node to bus 1; Part B extends the same function with a master chain of
one gain plus one true-peak limiter (B10), which is what closes exit-gate clause 3. A new test
runs only `assert_playback_matches_export` (audio.rs:2894-2937) on it — nine windows at 1e-6 plus
the frame-5 seek — together with the (c) tail and seek-alignment checks. A second variant keeps
the legacy `audio_eq` and `audio_limiter` on a second bus.

## 4. Part A agent

### 4.1 `upsert_audio_bus` prose and the pattern documentation (R13)

The `UpsertAudioBus | RemoveAudioBus` description arm (schema.rs:477-479) is replaced by:

> Audio buses route each track to at most one bus and carry an ordered effect chain. Bus
> effects must use one of audio_gain, audio_eq, audio_compressor, audio_ducking, audio_limiter,
> audio_parametric_eq, audio_gate, or audio_true_peak_limiter. Prefer
> audio_parametric_eq (high-pass, low and high shelves, four peaking bands), audio_compressor
> (peak or RMS detection, soft knee, lookahead), audio_gate, and audio_true_peak_limiter, whose
> true_peak mode measures the inter-sample peak with a 4x oversampled detector. audio_eq and
> audio_limiter are legacy fixed-crossover and hard-clamp nodes kept for existing projects; do
> not add them to a new chain. Every node takes bypass, 0 or 1; bypass, detector and true_peak
> accept only hold keyframes. The nodes of one chain may declare at most 20 ms of
> lookahead_milliseconds in total, and that parameter cannot be keyframed at all. Every other
> numeric control supports the same fixed-point keyframe curves as clip effects, on project
> frames; frequency and Q interpolate linearly in their integer unit, not logarithmically.
> Ducking reads the listed sidechain tracks before bus processing. Unrouted tracks feed the
> master directly.

`effect_documentation()` (schema.rs:502-566) gains
**`audio_parametric_eq_pattern_documentation()`** in the `color_curves_pattern_documentation()`
style (rationale at schema.rs:510-514, helper at :668, call at :516) and the matte
shared-legend style (schema.rs:524-525, :543): the twelve
`band{1..4}_{hertz, gain_tenth_db, q_hundredths}` rows are replaced by one sentence naming the
generating pattern with the bounds read from the descriptor, rather than enumerated four times
into each of the five effect tools. The other seven EQ parameters, the four compressor
additions, the gate, the limiter, and the single shared `bypass` row are enumerated normally.

### 4.2 Part A registry effect (A17)

Part A adds **no tool**: the count stays **126** (50 generated operations + 76 inspectors) and
`INSPECTOR_TOOL_NAMES` stays `[&str; 76]` (schema.rs:16). Only the five effect tools'
descriptions grow — `AddEffect | InsertEffect | SetEffectParam | SetEffectKeyframes |
ClearEffectKeyframes` (schema.rs:388-397).

**Only `description_bytes` moves.** The `Operation` schema embeds `Effect.parameters` as an
untyped map (model.rs:172), so no descriptor addition changes it: `input_schema_bytes` stays
**1 186 449 B, byte-identical**, and `description_bytes` moves from **96 840 B** to roughly
103 000 B (40 rows at about 45 bytes across five tools is about 9 KB before the pattern helper
and about 6 KB after it). The exact figure is regenerated at `server.rs:21075-21082`, its
explanatory comment at `server.rs:21071-21073` is rewritten with it, and both land in the M36
Part A row. Asserting the input-schema figure unchanged is a stronger and cheaper check than
regenerating the total.

**The served triple is asserted byte-identical, not regenerated:** the seven served tools do not
embed the `Operation` schema (mcp_server.rs:2504-2507), so `7 / 5_660 / 3_510 / 998` stay exactly
as they are at `server.rs:21064` and `tests/mcp_server.rs:2508-2511`.
`tests/mcp_server.rs:2479` (`assert_eq!(tools.len(), 7, "AU1 adds no served tool")`) keeps its
value and has its message rewritten to name AU2a.

`plan_audio_normalization` is untouched: the handler (server.rs:8153-8207),
`verified_normalization_operation` (:8340-8402), and `normalization_bus` (:8424-8503) keep their
behaviour and keep emitting `audio_compressor` + `audio_gain` + the legacy `audio_limiter`
(:8487-8493). Because the compressor's four additions are neutral-preserving and `W == 0` takes
the AU1 branch verbatim (§3.3), the plan's four-iteration convergence loop and the g2/g3 rendered
loudness windows (`benchmarks/auto-edit/v5/manifest.json:129-131`, :177-179;
`kinewright-eval.rs:1441-1445`, :1573-1577) are unchanged.

---

# Part B — Bus and master control

---

## 5. Part B core model and media

### 5.1 `AudioBus.gain_tenth_db` (D5, R19)

```rust
pub struct AudioBus {
    pub id: AudioBusId,
    pub name: String,
    pub tracks: Vec<TrackId>,
    /// AU2 §5.1: post-effects bus fader in integer tenths of a decibel,
    /// inclusive range `AUDIO_BUS_GAIN_MIN..=AUDIO_BUS_GAIN_MAX`.
    #[serde(default, skip_serializing_if = "i32_is_zero")]
    #[schemars(default)]
    pub gain_tenth_db: i32,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[schemars(default)]
    pub effects: Vec<Effect>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[schemars(default)]
    pub ducking_sidechain_tracks: Vec<TrackId>,
}
```

**Sixteen** `AudioBus { … }` struct literals in expression position gain `gain_tenth_db: 0`
(audio.rs:2310 is the signature `fn ducked_bus() -> AudioBus {`, not a literal, and model.rs:479
is the definition):

`app/src/mixer_ui.rs:794`; `core/tests/contracts.rs:257, 3029, 3043, 3050, 3062`;
`agent/src/server.rs:8495`; `media/src/audio.rs:1805, 1829, 1856, 1886, 2311, 2348, 2615, 2780,
2814`.

None use `..Default::default()`, so all sixteen are edited by hand.

### 5.2 `AudioMaster` and `SetAudioMaster` (D6)

```rust
/// The master chain (AU2 §5.2). A neutral master is omitted from the wire.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AudioMaster {
    #[serde(default, skip_serializing_if = "i32_is_zero")]
    #[schemars(default)]
    pub gain_tenth_db: i32,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[schemars(default)]
    pub effects: Vec<Effect>,
}
impl AudioMaster {
    /// `const` so `AudioMix::is_empty` stays `const` (R20).
    #[must_use] pub const fn is_neutral(&self) -> bool {
        self.gain_tenth_db == 0 && self.effects.is_empty()
    }
}

/// Replace the whole master chain (AU2 §5.2). Idempotent full set.
SetAudioMaster { master: AudioMaster },
```

### 5.3 `PanLaw` and `SetPanLaw` (D7, R15, R16, A31)

```rust
/// How `TrackMix.pan_percent` becomes per-channel gains (AU2 §5.7).
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PanLaw {
    /// Centre is the exact identity; the law never boosts (AU1 §3.1).
    #[default]
    Balance,
    /// Constant power: centre is -3.0103 dB on both channels, L^2 + R^2 == 1.
    ConstantPower,
}
impl PanLaw {
    // The same allow every `Copy` serde predicate in model.rs carries
    // (model.rs:362-363, :368-369, :373-374) under clippy pedantic + `-D warnings`.
    #[allow(clippy::trivially_copy_pass_by_ref)]
    #[must_use] pub const fn is_balance(&self) -> bool { matches!(self, Self::Balance) }
}

/// Set the document-level pan law (AU2 §5.7). Idempotent full set.
SetPanLaw { law: PanLaw },
```

### 5.4 `AudioMix`, `is_empty`, and `next_bus_id` (R20, N1)

```rust
pub struct AudioMix {
    pub buses: Vec<AudioBus>,
    pub tracks: Vec<TrackMix>,
    #[serde(default, skip_serializing_if = "AudioMaster::is_neutral")]
    #[schemars(default)]
    pub master: AudioMaster,
    #[serde(default, skip_serializing_if = "PanLaw::is_balance")]
    #[schemars(default)]
    pub pan_law: PanLaw,
}
impl AudioMix {
    /// R20: stays `const`. Derived `PartialEq` is not `const`, so the law is
    /// tested with `matches!`, never `==`.
    #[must_use] pub const fn is_empty(&self) -> bool {
        self.buses.is_empty() && self.tracks.is_empty()
            && self.master.is_neutral() && matches!(self.pan_law, PanLaw::Balance)
    }
    /// AU2 §5.4: the next unused bus id, `max + 1`.
    #[must_use] pub fn next_bus_id(&self) -> AudioBusId;
    #[must_use] pub fn bus(&self, id: AudioBusId) -> Option<&AudioBus>;
    #[must_use] pub fn bus_for_track(&self, track: TrackId) -> Option<AudioBusId>;
}
```

Widening `is_empty` is safe: its callers are the serde skip on `Document.audio_mix`
(model.rs:793-795), `contracts.rs:3092`, and `au1_core.rs:182`/`:266`. The seek-preroll predicate
reads `buses` directly (audio.rs:1120-1122), so AU1's F1 cannot recur, and `mixer_document()`
(mixer_ui.rs:763-813) does not touch it.

`next_bus_id` replaces the inline allocator in `normalization_context`
(**server.rs:8313-8322**) and is used by "+ Bus" (§6.5). **There is no document-wide effect-id
helper** (N1): the chain editor allocates `max(chain effect ids) + 1` per chain, as
`next_effect_id(clip)` (inspector_ui.rs:302-311) does per clip.
`color_status::next_effect_id` (color_status.rs:1642-1653) and `normalization_context`'s
effect-id scan (server.rs:8323-8332) are unchanged.

New Part B `OpError` variants:

| variant | message |
| --- | --- |
| `AudioBusGainOutOfRange { bus, gain_tenth_db }` | `audio bus {bus} gain is {gain_tenth_db} tenth-dB, outside the inclusive range -600..=120` |
| `AudioMasterGainOutOfRange { gain_tenth_db }` | `audio master gain is {gain_tenth_db} tenth-dB, outside the inclusive range -600..=120` |
| `DuplicateAudioMasterEffect { effect }` | `audio master chain has more than one effect with id {effect}` |
| `VisualEffectOnAudioMaster { effect }` | `effect {effect} is not an audio effect and cannot sit on the audio master chain` |
| `AudioMasterDuckingUnsupported` | `audio_ducking has no sidechain on the master chain and cannot be used there` |
| `AudioMasterKeyframeOutsideProject { effect, name, at, duration }` | `audio master effect {effect} keyframes {name} at frame {at}, outside the project duration {duration}` |
| `AudioMasterLookaheadExceeded { milliseconds }` | `audio master chain declares {milliseconds} ms of lookahead, beyond the 20 ms chain budget` |

`AUDIO_BUS_GAIN_MIN/MAX` and `AUDIO_MASTER_GAIN_MIN/MAX` are `-600`/`120`, live in `model.rs`
beside `TRACK_MIX_GAIN_MIN` (model.rs:511-518), are re-exported from `lib.rs` (lib.rs:155-156),
and are pinned equal to the `audio_gain` descriptor bounds by a core test (the AU1 precedent,
`tests/au1_core.rs:359-368`).

A new `validate_audio_master(doc)` mirrors `validate_audio_bus`: gain range, effect-id uniqueness
**scoped to the master chain** (`DuplicateAudioMasterEffect`, N1), `is_audio_effect`,
`validate_effect`, **`audio_ducking` rejected outright** (`AudioMasterDuckingUnsupported`,
because a master chain has no sidechain), curve validity, `at < doc.duration`, keyframe value
range, the two §2.2 keyframe rules, and the 20 ms budget. `validate_audio_mix`
(operation.rs:3924-3943) calls it after the per-bus loop; `validate_audio_bus` gains the gain
range check before its effect loop.

Both operations are declared immediately after `RemoveAudioBus` (operation.rs:81-83) and before
`AddTrack`, so generated tools land as `… upsert_audio_bus, remove_audio_bus, set_audio_master,
set_pan_law, add_track …`. `Operation` goes 53 → 55; generated tools 50 → 52. Apply
(`set_audio_master`): validate, then replace `doc.audio_mix.master`; a neutral value is stored as
the default and disappears from the wire. Apply (`set_pan_law`): store the law; `Balance`
disappears from the wire. Exhaustive-match sites gaining two arms: `apply_unchecked`
(operation.rs:922-927), `operation_tool_name` (schema.rs:255-274, after the `RemoveAudioBus` arm
at :270), `operation_status` (app.rs:1717-1736).

### 5.5 Part B contract tests

`tests/contracts.rs:177-489` round-trip list gains `SetAudioMaster` and `SetPanLaw` after
`RemoveAudioBus` (:273), and the `UpsertAudioBus` literal (:256-272) gains `gain_tenth_db: -30`.
`tests/contracts.rs:3009-3092` gains an out-of-range bus gain among its rejected buses.
`tests/au2_core.rs` gains one test per §7 Part B item.

### 5.6 Stage order, `L_master`, and seek preroll (D6, R6)

```
clip shaping → track stage: gate → gain → pan by AudioMix.pan_law
→ routing: sidechain taps read the POST-track-stage, PRE-delay signal
→ per bus: effects in order → bus gain → alignment pad to L_bus
→ unrouted path: plain delay to L_bus
→ master sum → master gain → master effects in order → alignment pad to L_master
→ master limiter clamp +/-1.0 (unchanged) → master meter (unchanged)
```

`L_master = chain_lookahead_milliseconds(&master.effects)`; the total is
`graph_latency_frames(&lookahead, fs)` (§3.6). The mechanism is Part A's, reused unchanged.

`needs_seek_preroll` (**audio.rs:1120-1122**) becomes

```rust
(!document.audio_mix.buses.is_empty() || !document.audio_mix.master.effects.is_empty())
    && project_from > TimeCode::ZERO
```

A document with no buses and one master true-peak limiter — the first document the "+ Effect"
menu on the master produces — has stateful nodes and must warm them. A test opens a mixer at
frame 5 on such a document and asserts `decode_from == TimeCode::ZERO`;
`a_track_mix_only_document_needs_no_seek_preroll` (audio.rs:2765-2789) is unchanged and still
passes.

### 5.7 Gains and the pan law

Bus and master gain reuse the `TrackStageRuntime` ramp discipline (audio.rs:615-765,
`TRACK_MIX_RAMP_MILLISECONDS = 5`, audio.rs:31-32) in scalar form:

```rust
struct GainRamp { target: f32, current: f32, ramp_start: f32, ramp_index: usize }
```

Construction is settled (`current == target`, `ramp_index == ramp_frames`), so export and a
freshly opened playback mixer never ramp and steady-state parity is untouched. A retarget that
differs from `current` stores `ramp_start = current` and restarts at index 0; sample frame `i`
uses `ramp_start + (target - ramp_start) * ((i+1) as f32 / ramp_frames as f32)` and stores
`current = target` exactly at `i == ramp_frames - 1`. Playback-only transient state, not
observable through any facet.

`pan_channel_ratios` (audio.rs:589-592) takes the law and computes in **f64**, casting once
(R16, matching "analysis in f64, mix in f32"):

```
Balance:        pan = pan_percent as f64 / 100.0
                [1.0 - pan.max(0.0), 1.0 + pan.min(0.0)]      // unchanged, AU1 §3.1
ConstantPower:  theta = (pan_percent as f64 / 100.0 + 1.0) * PI / 4.0
                [cos(theta), sin(theta)]
```

The three cardinal positions are **special-cased to exact ratios** so `ConstantPower` does not
lose AU1's "hard left yields the left channel at unity and the right silent": `pan_percent ==
-100` → `[1.0, 0.0]`, `== 0` → `[SQRT_1_2, SQRT_1_2]`, `== +100` → `[0.0, 1.0]`. Without the
special case `cos(PI/2)` in f32 is `-4.371e-8`, leaking `-147 dBFS` into the far channel.

**Channel rules (R15, A28).** AU1's rules are unchanged and the law changes neither:
`track_stage_parameters` returns `[gain, gain]` when `channels < 2` (audio.rs:608-610) — **a
one-channel device applies no pan under either law** — and channels beyond the first two take the
unpanned `gain` (AU1 §3.1, AU1:253-255). Under `ConstantPower` that means a mono device hears a
hard-panned track `+3.01 dB` above its stereo per-channel level, and a device with more than two
channels sits channels 2+ `3.01 dB` above the stereo pair at centre — a step `Balance` does not
have. Playback/export parity is claimed at two channels only. Both figures are recorded in
§6.10.

A live pan-law switch needs no new machinery: `TrackStageRuntime::retarget` (audio.rs:650-671)
recomputes `track_stage_parameters` and compares `target == self.current` exactly, so a law
change makes every stage's target differ and every stage ramps over the 5 ms constant.
`SetPanLaw` only has to reach the processor through §5.8's update path.

### 5.8 Live structural diff and the meter table (D8, R8, A13, A33, A34)

```rust
impl AudioMixProcessor {
    /// AU2 §5.8: apply a document that may differ in track mix, bus chains,
    /// the master chain, or the pan law, without stopping playback.
    /// The caller guarantees `AudioMix::lookahead_milliseconds()` is unchanged.
    pub(crate) fn update_audio_mix(&mut self, document: &Document);
}
```

Replaces `AudioMixProcessor::update_track_mix` (audio.rs:845-851) as the live entry point. The
two forwarders between the worker and the processor are renamed with it:
**`AudioMixer::update_track_mix` (audio.rs:1225-1227)** and
**`AudioRuntime::update_track_mix` (audio.rs:1404-1406)** both become `update_audio_mix`, and
`Worker::update_audio_mix` (engine.rs:1428-1437) calls through them.

In order:

1. Retarget every `TrackStageRuntime` (`retarget`, audio.rs:650-671), which now also folds the
   pan law.
2. Recompute `routed_tracks` — **on every update**.
3. For each bus in the new document, in the new document order, look up the existing runtime by
   `AudioBusId`. **Structure** is the ordered list of `(EffectId, effect name, static
   lookahead_milliseconds)`. Same structure → retarget parameters in place, preserving filter
   state, envelopes, delay lines, and RMS windows. Different structure, or no existing runtime →
   build a fresh `AudioBusRuntime`: state reset, a momentary discontinuity accepted for a
   structural edit. Runtimes for buses no longer present are dropped.
4. The same rule for the master chain runtime.
5. Bus gain and master gain retarget through the 5 ms ramp of §5.7.

**A retarget invalidates the per-node parameter cache unconditionally (A13).** `retarget` sets a
`dirty` flag (equivalently, bumps the runtime's parameter epoch, which §3.2 makes part of the
cache key); the next `process_frame` clears it and recomputes. Without this, a live parameter
edit would not be heard until the next project-frame boundary — 41.7 ms at 24 fps, **100 ms at
10 fps** (the rate `processor_document` uses, audio.rs:1756), a whole second on a 1 fps
timelapse — an unbounded second term on top of AU1's "about one second after the gesture"
ring-fill figure (MEDIA-POLICY.md:116-119).

**Seamless bus creation.** When a track moves from the unrouted path to a new bus, the unrouted
path's delay line is **left to drain** rather than cleared, so its `L_bus` tail plays out while
the new chain's line fills and the join has no gap.

**Meter table (R8, A33).** `MixMeters::for_document` allocates fresh zeroed `MeterState`s
(audio.rs:210-226), and `MixerMeterLevels::advance` (mixer_ui.rs:116-148) only raises while
playing, so rebuilding the table on every update would zero every meter on every frame of a
fader drag. Rule: **the table is rebuilt only when `MixMeters::matches_document(&document)` is
false** — that is, when the `(track id, bus id, chain-and-effect-id)` key set differs from the
installed table. A parameter-only retarget leaves the installed table and its accumulated peaks
alone. When a rebuild is needed the worker installs the new table through `install_mix_meters`
(engine.rs:1602-1605) and attaches it with `AudioMixProcessor::attach_meters` in the same call.

**The latency guard (A34).** `L` is fixed for a processor's life, so a document whose lookahead
differs cannot be applied to a running one. `Worker::update_audio_mix` (engine.rs:1428-1437)
compares `self.document.audio_mix.lookahead_milliseconds()` with the incoming document's and, if
they differ, falls back to `set_document` (a pause and re-cue) rather than retargeting —
defensive, because the app predicate (§6.8) already refuses the live path in that case.

### 5.9 `Analysis::mix_spectrum` (D11, F12, A5, A23, A24)

```rust
// core media.rs, beside MixLevelReport (media.rs:1105-1118)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum MixSpectrumPoint { Master, Track(TrackId), Bus(AudioBusId) }

// The three types below carry `MixLevelReport`'s derive line verbatim
// (media.rs:1105-1108).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MixSpectrumRequest { pub range: Option<Range<TimeCode>>, pub point: MixSpectrumPoint }

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MixSpectrumReport {
    pub range: Range<TimeCode>,
    pub point: MixSpectrumPoint,
    pub sample_rate: u32,
    pub sample_frames: u64,
    pub segments: u32,
    /// 31 ISO third-octave bands, low to high.
    pub bands: Vec<SpectrumBand>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SpectrumBand {
    /// ISO nominal centre frequency in tenths of a hertz.
    pub center_hertz_tenths: u32,
    /// Band level in hundredths of a dBFS; `None` when the band measured silent.
    pub level_dbfs_hundredths: Option<i32>,
    /// True when the band is narrower than the analysis window's main lobe, so
    /// its reading is dominated by leakage from its neighbours (AU2 §5.9).
    pub window_limited: bool,
}

pub trait Analysis {
    /// AU2 §5.9: the third-octave spectrum of one mix point over a project
    /// range, measured through the real mix path.
    fn mix_spectrum(&self, document: &Document, request: &MixSpectrumRequest)
        -> Result<MixSpectrumReport, MediaError> {
        let _ = (document, request);
        Err(MediaError::NotImplemented)
    }
}

// MediaError gains one variant (A23), so the rejection is genuinely typed and
// not a formatted `Backend` string.
#[error("mix spectrum needs at least {required} sample frames; got {sample_frames}")]
MixSpectrumRangeTooShort { sample_frames: u64, required: u64 },
```

The 31 `center_hertz_tenths` values, in order: 200, 250, 315, 400, 500, 630, 800, 1000, 1250,
1600, 2000, 2500, 3150, 4000, 5000, 6300, 8000, 10000, 12500, 16000, 20000, 25000, 31500, 40000,
50000, 63000, 80000, 100000, 125000, 160000, 200000 — nominally 20 Hz through 20 kHz. Band edges
are the exact third-octave edges around `f_c = 1000 * 2^(k/3)` for `k` in `-17..=13`:
`[f_c * 2^(-1/6), f_c * 2^(1/6))`.

**Analysis.** Welch on the stem `mix_audio_stems` produces — the same path `measure_mix_levels`
(export.rs:941-1011) uses, so the measurement runs through the real graph at 48 kHz with §3.7's
head-and-tail trim already applied. **16 384-point Hann window, hop 8 192 (50 % overlap)**,
trailing partial segment discarded, power averaged across every segment and every channel. Bin
width `48 000 / 16 384 = 2.93 Hz`; Hann main lobe 4 bins = **11.72 Hz**.

**DFT convention and normalisation (A5).** The transform is the **unnormalised forward DFT**,
`X_k = Σ_n w[n] x[n] e^{-2πikn/N}`, with no `1/N` in the forward direction — the convention the
in-house radix-2 implementation uses. Under it Parseval gives
`Σ_k |X_k|² = N · Σ_n (w[n]x[n])²`, so the one-sided power estimate is

```
S_k = 2 * |X_k|^2 / (N * sum(w[n]^2))     for 1 <= k < N/2
S_k =     |X_k|^2 / (N * sum(w[n]^2))     for k == 0 and k == N/2
```

The `N` is essential: without it every band reads `10*log10(N) = +42.14 dB` high. Band power is
the **overlap-weighted sum of `S_k` over the band's edges** — each bin contributes in proportion
to the fraction of its 2.93 Hz width that falls inside the band — never a "bin centre inside the
band" test, which would leave the 20 Hz and 40 Hz bands empty. `level = 10*log10(P_band / 0.5)`,
so a full-scale sine inside one band reads exactly `0.00 dBFS`. `P_band` below `1e-12` reports
`None`.

`window_limited` is `true` for the five bands narrower than the main lobe — **20 Hz (4.56 Hz
wide), 25 (5.74), 31.5 (7.24), 40 (9.12), and 50 (11.49)** — and false from 63 Hz (14.47 Hz) up.

**Minimum range (A23).** Welch needs two segments: `2 * 16 384 - 8 192 = 24 576` sample frames =
**512 ms** at 48 kHz. A shorter range returns
`Err(MediaError::MixSpectrumRangeTooShort { sample_frames, required: 24_576 })`, never a
degenerate spectrum.

FFT: an in-house iterative radix-2 Cooley-Tukey over `f64` complex pairs with a bit-reversal
permutation, in a new `crates/kinewright-media/src/spectrum.rs`. No new dependency — the
workspace pins every version with `=` (Cargo.toml:19-42) and media carries no DSP crate.
`FfmpegMediaEngine` implements `mix_spectrum` via a new `export::measure_mix_spectrum`;
`BaselineProofAnalysis` (`app/src/color_qc_ui.rs:445-453`) delegates explicitly; the seven other
`Analysis` doubles inherit the default body above (facts-agent §6).

### 5.10 Part B media evidence (D12 e–f)

**(e) Pan law.** `L^2 + R^2 == 1` within 1e-6 at every integer pan; the three cardinal positions
are exact (`[1,0]`, `[SQRT_1_2, SQRT_1_2]`, `[0,1]`, asserted with `==`); centre is
`-3.0103 dB ± 0.001`; `Balance` is bit-identical to AU1; a mono processor returns `[gain, gain]`
under both laws and a four-channel processor leaves channels 2 and 3 at `gain` under both.

**(f) Serialization and load.** A neutral `AudioMaster` and `PanLaw::Balance` are omitted from
the serialized document; a document only ever set neutral serializes byte-identically to one
never touched. Hand-edited rejections at load: out-of-range bus and master gain, a visual effect
on the master, a duplicate master effect id, `audio_ducking` on the master, a master keyframe at
or past `duration`, a chain declaring 21 ms of lookahead, a `Linear` curve on `bypass`, and any
curve on `lookahead_milliseconds`.

Plus: parity through the full chain **including the master** (§3.9(d) extended, B10); bus and
master gain ramping over 240 sample frames at 48 kHz and settling exactly, with a fresh processor
never ramping; the live diff preserving state on a parameter-only edit, rebuilding on an
insertion, a removal and a reorder, a bypass flip retargeting in place, and a parameter retarget
being audible on the **next sample frame** rather than the next project frame (A13);
`MixMeters::matches_document` true after ten consecutive live gain updates (`Arc::ptr_eq` on the
installed table) and false after a structural edit; `mix_spectrum` of a 1 kHz tone peaking in the
1 000 Hz band and at least 40 dB down two octaves away, **and the same at 80 Hz** to prove the
low end is not empty; a full-scale sine inside one band reading `0.00 dBFS` within 5 hundredths;
the in-house FFT matching a direct DFT at `N = 64` within 1e-9; a 400 ms range rejected with
`MixSpectrumRangeTooShort`; `window_limited` true for exactly the five named bands.

## 6. Part B agent and app

### 6.1 Generated tools (A14)

Annotations (schema.rs:378-384) become `.idempotent(matches!(name.as_str(), "set_clip_audio" |
"set_track_mix" | "upsert_audio_bus" | "set_audio_master" | "set_pan_law"))`. The destructive
list is unchanged.

**`SetTrackMix`'s description becomes law-neutral (A14).** `schema.rs:438-440` today writes
"pan_percent is an integer in -100..=100 **using a balance law** (0 is an exact identity, -100
silences the right channel, 100 silences the left)". Under `PanLaw::ConstantPower` the "0 is an
exact identity" half is false, so the sentence becomes:

> pan_percent is an integer in -100..=100; -100 is hard left and 100 is hard right. How a
> position becomes per-channel gains is a document-level setting, `set_pan_law`: under the
> default balance law 0 is an exact identity, and under constant_power 0 is -3.01 dB on both
> channels.

The core doc comment at `operation.rs:100` ("Balance law, see AU1 §3.1") gains the same
correction, and `set_track_mix_schema_documents_ranges_and_the_balance_law` (schema.rs:896-905),
which asserts `description.contains("balance law")`, is updated with it.

`SetAudioMaster` description arm:

> The master chain sits after the bus sum: master gain, then the effects in order, then the
> single hard clamp to -1.0..=1.0. gain_tenth_db is an integer number of tenths of a decibel in
> -600..=120. The same audio effect names, ranges, curves, keyframe rules and 20 ms lookahead
> budget apply as on a bus, except audio_ducking, which has no sidechain at master and is
> rejected. The operation replaces the whole master chain; sending a zero gain and an empty
> effect list removes it.

`SetPanLaw` description arm:

> The pan law decides how each track's pan_percent becomes per-channel gains. balance is the
> default and leaves centre an exact identity, so a centred document renders bit-identically.
> constant_power places centre at -3.01 dB on both channels and keeps L^2 + R^2 equal to 1 at
> every position; switching to it changes the output of every panned and centred track. A
> one-channel output device applies no pan under either law, and channels beyond the first two
> take the unpanned gain. The law is a document-level setting; there is no per-track override.

The `UpsertAudioBus | RemoveAudioBus` arm (rewritten in Part A, §4.1) gains one sentence: "Each
bus also carries a post-effects fader, gain_tenth_db, in tenths of a decibel in -600..=120."

### 6.2 `get_audio_spectrum`

```rust
/// AU2 §6.2: the third-octave spectrum of one mix point.
#[derive(Debug, Deserialize, JsonSchema)]
struct AudioSpectrumArgs {
    /// Optional project-frame range start; defaults to frame 0.
    #[serde(default)] start_frame: Option<TimeCode>,
    /// Optional project-frame range end; defaults to the timeline duration.
    #[serde(default)] end_frame: Option<TimeCode>,
    /// Measure this track's post-track-stage stem instead of the master.
    #[serde(default)] track: Option<TrackId>,
    /// Measure this bus's post-chain stem instead of the master.
    #[serde(default)] bus: Option<AudioBusId>,
}
```

Wiring follows `get_audio_levels` exactly (facts-agent §5): the name joins
`INSPECTOR_TOOL_NAMES` (schema.rs:16, 74-78) immediately after `"get_audio_levels"`, making the
array `[&str; 77]`; the `Tool::new` sits between `get_audio_levels` (server.rs:9895-9900) and the
`Tool::new(` that opens at server.rs:9901, with `.with_annotations(read_only())`; the dispatch
arm follows the `get_audio_levels` arm (server.rs:966-969); the handler is
`fn audio_spectrum(&self, args: &AudioSpectrumArgs)`. No `CAPABILITY_KIND_OVERRIDES` entry —
`get_` infers `Inspector` (runtime.rs:153-182).

Handler: snapshot; both `track` and `bus` set → `error_text("get_audio_spectrum takes at most one
of track and bus")`; neither → `MixSpectrumPoint::Master`; the range is built and validated
exactly as `audio_levels` does (`start >= end` → `error_text`, one bound fills the other); an
analysis error, including `MixSpectrumRangeTooShort`, → `error_text(format!("could not measure
the mix spectrum: {error}"))`.

Text, no trailing newline:

```
mix_spectrum range={start}..{end} point={point} sample_frames={n} segments={s} levels in dBFS hundredths
band 20 {level} window_limited
band 25 {level} window_limited
band 31.5 {level} window_limited
band 40 {level} window_limited
band 50 {level} window_limited
band 63 {level}
...
band 20000 {level}
```

`{point}` renders `master`, `track {id}`, or `bus {id}`. A centre renders `tenths / 10` when
`tenths % 10 == 0` and `format!("{}.{}", tenths / 10, tenths % 10)` otherwise, so band 3 reads
`31.5`. `{level}` uses `render_optional_hundredths` (server.rs:10690-10694) and reads `none` for
a silent band. The ` window_limited` suffix appears only on the five flagged bands. Structured
output is `success_structured(text, json!({ "timeline_revision": revision.0, "report": report
}))`.

Tool description:

> Measure the third-octave spectrum of one mix point through the real mix path: 31 ISO nominal
> bands from 20 Hz to 20 kHz, reported in hundredths of a dBFS, where a full-scale sine inside
> one band reads 0. Measure the master by default, or one track's post-track-stage stem, or one
> bus's post-chain stem. The five bands below 63 Hz are narrower than the analysis window and are
> flagged window_limited. The range must cover at least 24576 sample frames (512 ms at 48 kHz).
> Omit both frame bounds to measure the whole timeline. Use it to justify an EQ move before
> proposing one. This capability is read-only and produces no edit operations.

### 6.3 Compact state rendering (A32)

`render_audio_mix` (render.rs:622-653) no longer returns early on an empty bus list: it returns
only when there are no buses, the master is neutral, and the pan law is `Balance`. Three
renderings, in this order:

```
audio_pan_law=constant_power                                     // only when not Balance
audio_buses:
  audio_bus {id} {name:?} tracks={ids} gain={tenth_db} sidechain={ids|none} effects={…}
audio_master gain={tenth_db} effects={render_effects}            // only when not neutral
```

` gain={tenth_db}` is inserted between `tracks=` and `sidechain=` **only when non-zero**. This is
the **first conditional mid-line field in `render.rs`** and the contract owns it: the nearest
precedents, `track_mix_fields` (render.rs:432-443) and `render_clip_audio` (render.rs:409-421),
both omit a whole trailing suffix when their subject is neutral, and every field of
`render_audio_mix`'s existing bus line (render.rs:640-652) is unconditional. The reason to omit
rather than always print is that a zero fader is the overwhelming default and the bus line is
already the longest in the compact state; the reason it is safe is that no test pins
`audio_buses:` text (facts-agent §3).

`render_effects` (render.rs:896-935) is shared with the clip lines and is unchanged, so the clip
goldens do not move; the `timeline_state_matches_the_compact_golden_rendering` fixture
(render.rs:1055-1069) has no buses and a neutral master, so it does not move either.

### 6.4 Moved assertions (R26, A17)

| site | from | to |
| --- | --- | --- |
| `schema.rs:16` | `INSPECTOR_TOOL_NAMES: [&str; 76]` | `[&str; 77]` |
| `schema.rs:74-78` | list ends `"get_audio_levels"` | `"get_audio_spectrum"` immediately after it |
| `schema.rs:716-788` | 50 ordered operation tool names (:726-775) | 52; `set_audio_master`, `set_pan_law` after `remove_audio_bus` (:737), in `Operation` declaration order |
| `schema.rs:378-384` | `.idempotent(set_clip_audio \| set_track_mix)` | adds `upsert_audio_bus`, `set_audio_master`, `set_pan_law` |
| `schema.rs:438-440` | `SetTrackMix` description says "using a balance law" | law-neutral text naming `set_pan_law` (§6.1) |
| `schema.rs:896-905` | `set_track_mix_schema_documents_ranges_and_the_balance_law` asserts `contains("balance law")` | updated with the new sentence |
| `schema.rs:920-945` | `operation_exhaustiveness_guard_requires_new_variants_to_be_acknowledged` | adds `SetAudioMaster` and `SetPanLaw` spot checks |
| `operation.rs:100` | doc comment "Balance law, see AU1 §3.1" | names the document-level law |
| `server.rs:19203` | `INSPECTOR_TOOL_NAMES.len() == 76` (in `cc5_matte_tools_are_registered_read_only_inspectors` :19183) | 77 |
| `server.rs:21071-21073` | the comment spelling out AU1's 126 / 1,303,967 / 1,186,449 / 96,840 breakdown | regenerated in **both** parts |
| `server.rs:21075-21082` | `(1_303_967, 5_660)` | Part A: `input_schema_bytes` **1 186 449 unchanged**, `description_bytes` regenerated; Part B: both regenerated; **served 5 660 asserted unchanged** in both |
| `tests/mcp_server.rs:2479` | `assert_eq!(tools.len(), 7, "AU1 adds no served tool")` | value stays 7; the message names AU2 |
| `tests/mcp_server.rs:2483-2511` | 126, 76, served 7 / 5 660 / 3 510 / 998 | Part A: 126 / 76 unchanged; Part B: 129 / 77; served triple **byte-identical** in both; the doc comment rewritten |
| `contracts.rs:177-489` | round-trip list | two entries after `RemoveAudioBus` (:273) |
| `app.rs:1717-1736` | `operation_status` | two arms (§6.8) |
| `operation.rs:922-927` | `apply_unchecked` | two arms |
| `docs/M36:107-108` | AU1 rows | Part A row (126 tools, unchanged input schemas, new description bytes) and Part B row (129) |
| `runtime.rs:17-25` | `COMPACT_TOOL_NAMES: [&str; 7]` | unchanged |
| `schema.rs:121` | `UNGENERATED_OPERATION_VARIANTS: [&str; 3]` | unchanged |

52 generated operations + 77 inspectors = **129** internal tools.

### 6.5 Strips (R10, R23, A9)

The Mixer tab becomes **strips on the left (horizontally scrolling) and a chain pane on the
right** (§6.6), both painted by one shared free function (§6.6).

`track_strip` (mixer_ui.rs:248-294) gains, below its `Reset` row, a **`+ Bus`** small button
(recorded as `"add_bus:{track}"`), enabled only when the track carries audio
(`track_carries_audio`, mixer_ui.rs:672-677) and is not already routed; disabled with the reason
`"no audio"` or `"already routed"` as a tooltip. Pressed, it emits one `UpsertAudioBus` for
`AudioBus { id: document.audio_mix.next_bus_id(), name: <track caption>, tracks: vec![track],
gain_tenth_db: 0, effects: vec![], ducking_sidechain_tracks: vec![] }` — the bus is created
around the strip you are pointing at, which is what every other mixer control does.

`bus_strip` (mixer_ui.rs:454-492) becomes, top to bottom:

1. Name label, `type_size::CAPTION`, `TEXT_PRIMARY`. (≈ 14 px)
2. `tracks={caption list}` in `TEXT_MUTED`. (≈ 17 px)
3. **A node count** — `"4 nodes"`, or `"1 node"`, one line, never wrapped — replacing the
   `" → "`-joined chain text (mixer_ui.rs:469-478), which at 13 pt in a 72 px column wraps to
   roughly eight lines for a realistic AU2 chain. The full chain is the strip's **hover
   tooltip**. The `sidechain=` line **moves into the chain pane**. (≈ 17 px)
4. **One `meters_and_fader`-shaped row** (A9): the meter pair on the left and the vertical bus
   fader on the right, both `MIXER_FADER_HEIGHT` tall, with the dB readout under the fader
   **inside the same row** — the construction `track_strip` uses at mixer_ui.rs:362-399, whose
   doc comment (mixer_ui.rs:358-361) records why: stacking the meters above the fader made a
   374 px strip in AU1. The fader is a vertical `egui::Slider` over
   `AUDIO_BUS_GAIN_MIN..=AUDIO_BUS_GAIN_MAX`, `.integer()`,
   `slider_width = MIXER_FADER_HEIGHT`, `.custom_formatter` `"{:+.1} dB"`, `.custom_parser`
   `parse_gain_db` (mixer_ui.rs:699-705), `.update_while_editing(false)`; recorded as
   `"bus_fader:{id}"`. **The bus gain fader lives on the strip only**, never also in the pane.
   `meters_and_fader` (mixer_ui.rs:362-399) takes `mix: TrackMix` today, so it is generalised to
   take the slider range, formatter, parser, and current value directly, and `track_strip` calls
   it with the `TrackMix`-derived arguments. (120 px)
5. An `Edit` toggle (`"bus_edit:{id}"`) that sets `mixer_selection`. (`ICON_BUTTON` = 26 px)

The sentence "Bus controls arrive with AU2; the agent can edit buses today."
(mixer_ui.rs:487-490) is deleted. `master_strip` (mixer_ui.rs:495-500) keeps `MASTER` and its
meter pair and gains a node count, the same `meters_and_fader`-shaped row over
`AUDIO_MASTER_GAIN_MIN..=AUDIO_MASTER_GAIN_MAX` (`"master_fader"`), and an `Edit` toggle
(`"master_edit"`).

**Budget (R10, A9).** `measure_bus_strip` and `measure_master_strip` are added beside
`measure_track_strip` (mixer_ui.rs:1032-1052), and `a_track_strip_fits_the_mixer_dock`
(mixer_ui.rs:1062-1097, `const BUDGET: f32 = 240.0` at :1065) is extended to the worst case: a
bus with six nodes, a sidechain, and a non-zero gain. Arithmetic from the tokens
(`MIXER_STRIP_WIDTH 72`, `MIXER_FADER_HEIGHT 120`, `ICON_BUTTON 26`, theme.rs:78/:86/:89;
`item_spacing.y = space::HALF` = 2, mixer_ui.rs:510): rows `14 + 17 + 17 + 120 + 26 = 194 px`,
plus **four** gaps between five rows at 2 px = 8 px, **202 px** — inside 240 with the sidechain
line moved out and the chain text replaced. If the measured strip still exceeds 240,
`MIXER_STRIP_WIDTH` is raised for bus strips rather than silently overflowing the dock, and
DESIGN.md:328-333 ("strip 72 wide fits 240 tall") is edited either way.

### 6.6 The chain pane and the edit fold (F9/Q7, A4, A10, A16, N5/Q5)

**No floating window.** DESIGN.md's dock rule (:234-235) keeps applying. The Mixer tab's body is
a horizontal split: the strip scroll area on the left, and, when `mixer_selection` is `Some`, a
chain pane on the right, `MIXER_CHAIN_PANE_WIDTH` wide with its own vertical `ScrollArea`.

```rust
/// UI state, not document state (AU2 §6.6).
pub(crate) enum MixerSelection { Bus(AudioBusId), Master }
// on KinewrightApp: mixer_selection: Option<MixerSelection>
```

Cleared when the selected bus disappears from the focused document. Selecting a second chain
replaces the selection. Pane contents, top to bottom: a title (`Bus: {name}` or `Master`); for a
bus, one routing checkbox per audio-bearing document track and one sidechain checkbox per track;
the chain cards in `effects` order (§6.7); a `+ Effect` menu button; and, on the master, the
pan-law choice as two `selectable_value`s labelled "Balance" and "Constant power".

**One shared body function (A10).**

```rust
/// AU2 §6.6: the whole Mixer tab — strips and, when one is selected, the chain
/// pane — as a free function so the headless harness can drive both surfaces.
pub(crate) fn mixer_body(
    ui: &mut egui::Ui,
    document: &Document,
    selection: Option<MixerSelection>,
    peaks: &MixPeaks,
    playing: bool,
    levels: &mut MixerMeterLevels,
    edits: &mut InspectorEdits,
);
```

`mixer_panel` (mixer_ui.rs:188-207) calls it with `self.mixer_selection`, and `MixerHarness`
(mixer_ui.rs:1244-1300) calls it in place of the bare `mixer_strips` call at mixer_ui.rs:1296,
gaining a `selection` field. Without this, B18's fold pin — a frame that paints a strip control
**and** a pane control — cannot be written, because the harness builds only an `egui::Context`, a
`Document`, and `MixerMeterLevels` and cannot construct `&mut KinewrightApp`. `mixer_strips`
(mixer_ui.rs:211-241) keeps its shape and becomes the left half of `mixer_body`.

**Mutate-then-fold (A4).** Controls **never** push operations. They mutate an edited copy:

```rust
/// AU2 §6.6: one edited copy per touched chain, folded into one operation each
/// at the end of the frame.
#[derive(Default)]
pub(crate) struct MixerChainEdits {
    buses: BTreeMap<AudioBusId, AudioBus>,
    master: Option<AudioMaster>,
    /// True when any edit this frame came from a live drag.
    live: bool,
}
```

Every control — the strip fader, a card parameter, a bypass checkbox, a move, a remove, a
routing or sidechain checkbox, an insertion, the pan-law choice — takes `&mut MixerChainEdits`,
looks up (or clones in) its chain's entry, and mutates it. `drag_started` still calls
`edits.begin_gesture()`; `is_live_drag` sets `MixerChainEdits::live`. This is the opposite of
`matte_integer_control` (inspector_ui.rs:3045-3071), which pushes a finished operation at
inspector_ui.rs:3055-3069 — a control that pushes its own op is exactly the clobbering R9
forbids, and two pushed whole-chain `UpsertAudioBus` values cannot be folded after the fact.

**`mixer_body` drains `MixerChainEdits` into `InspectorEdits` after both the strips and the pane
have painted**, emitting exactly one `UpsertAudioBus` per touched bus (in `AudioBusId` order) and
at most one `SetAudioMaster`. For each folded operation it calls `push_live(op, key)` when
`live` is true and `push(op)` otherwise. This ordering matters because `push_live` sets the
coalesce key **only on the first operation of the frame** (`if self.operations.is_empty()`,
inspector_ui.rs:139-144) and `submit_inspector_edits` (inspector_ui.rs:737-787) reads exactly one
key per frame.

**Coalesce keys — one per chain (A16, N5/Q5):**

```
audio_bus:{id}        // e.g. "audio_bus:1"
audio_master
```

That is the AU1 precedent exactly: `track_mix_coalesce_key(track) = "track_mix:{id}"`
(mixer_ui.rs:636-637, pinned at mixer_ui.rs:919-931) covers all four `SetTrackMix` fields because
the operation is a whole-target idempotent set, and `UpsertAudioBus` is the same shape
(operation.rs:1407-1421). Per-chain granularity loses nothing: `submit_inspector_edits` appends
`#{gesture}` and `begin_edit_gesture` advances the counter on every `drag_started`, so two
consecutive drags of different controls on the same bus are already separate undo entries. When
two controls of one chain move in the same frame there is only one key to choose, so the fold's
key is unambiguous — the ambiguity a per-parameter scheme would create is the reason it is not
used.

### 6.7 Cards and controls (A30, N5/Q6)

**Display names (A30).** `effect_display_name` (inspector_ui.rs:3924-3933) gains all eight audio
names, chosen so the two limiters and the two EQs are never confusable:

| effect | display name |
| --- | --- |
| `audio_gain` | Gain |
| `audio_eq` | EQ (legacy) |
| `audio_compressor` | Compressor |
| `audio_ducking` | Ducking |
| `audio_limiter` | Limiter (legacy) |
| `audio_parametric_eq` | Parametric EQ |
| `audio_gate` | Gate |
| `audio_true_peak_limiter` | True-peak limiter |

The legacy pair matters because every document `plan_audio_normalization` has produced carries an
`audio_limiter` (`normalization_bus` always appends one, server.rs:8487-8493).

**Collapsed and expanded cards (N5/Q6).** A single `audio_parametric_eq` card is a header plus 18
controls plus a 96 px well — several hundred pixels inside a dock that leaves about 262 px
(mixer_ui.rs:1063-1065). Cards therefore carry UI state: **one card expanded at a time, the first
in the chain by default**, the rest collapsed. Clicking a collapsed header expands it and
collapses the previous one. The collapsed header row is at most `ICON_BUTTON` (26 px) tall and
holds, left to right: the display name, the `Bypass` checkbox, the gain-reduction readout (a
`MICRO` `TEXT_MUTED` `-{x:.1} dB`, blank for a node with no gain computer), `▲` and `▼`
`small_button`s (disabled at the ends), and a `Remove` `small_button` in `STATUS_DANGER` text.
`measure_chain_card` is added beside `measure_bus_strip` and pins the collapsed height at
`ICON_BUTTON`. An **expanded** card may exceed the pane and scrolls inside the pane's
`ScrollArea::vertical`; that is a recorded limit (§6.10).

An expanded card adds, under the header, a `horizontal_wrapped` of descriptor-driven controls,
one per parameter other than `bypass`, built by

```rust
fn mixer_parameter_control(
    ui: &mut egui::Ui, chain: MixerSelection, effect: &Effect, name: &str,
    label: &str, unit: MixerUnit, value: i64, edits: &mut MixerChainEdits);
```

which ranges its `DragValue` from the descriptor via `effect_descriptor(name).parameter(name)`
with the same `debug_assert` and inert `value..=value` fallback as `matte_parameter_range`
(inspector_ui.rs:3026-3043), calls `edits.begin_gesture()` on `drag_started`, and on
`changed && edited != value` **mutates the chain copy** (§6.6) — never pushing.

Unit formatters and parsers — the first Hz, ms, ratio, and Q formatters in the app:

| unit | display | parser accepts |
| --- | --- | --- |
| dB (tenth dB) | `format!("{:+.1} dB", v as f64 / 10.0)` → `+3.5 dB` | `parse_gain_db` (existing) |
| Hz | `v >= 1000` → `format!("{:.1} kHz", v as f64 / 1000.0)` → `1.2 kHz`; else `format!("{v} Hz")` → `250 Hz` | `1.2 kHz`, `1.2k`, `1200 hz`, `1200` |
| ms | `format!("{v} ms")` → `12 ms` | `12 ms`, `12` |
| ratio (hundredths) | `format!("{:.1}:1", v as f64 / 100.0)` → `4.0:1` | `4.0:1`, `4:1`, `4` |
| Q (hundredths) | `format!("Q {:.2}", v as f64 / 100.0)` → `Q 0.71` | `Q 0.71`, `0.71` |
| flag (`detector`, `true_peak`) | a two-item `selectable_value` pair, not a `DragValue` | — |

Parsers are case-insensitive, trim whitespace, and return `None` on anything else, so a bad entry
leaves the value alone.

An expanded `audio_parametric_eq` card carries a read-only magnitude well:
`allocate_exact_size(vec2(available_width, size::MIXER_EQ_CURVE_HEIGHT), Sense::hover())`;
`rect_filled(rect, radius::SM, LETTERBOX)`, `theme::paint_inset_well` (theme.rs:236-237, whose
doc comment reads "Darken the top of an inset well so it reads as recessed (light from above)"),
`rect_stroke(BORDER_SUBTLE)`; grid in `BORDER_SUBTLE` at ±24, ±12, 0 dB and 100 Hz / 1 kHz /
10 kHz; the curve as `Shape::line` of 96 points (the `CURVE_SAMPLES` figure,
curve_editor_widget.rs:46), stroke 1.6, `TEXT_PRIMARY`;
`x = rect.left() + rect.width() * log10(f/20) / log10(1000)` over 20 Hz–20 kHz; `y` maps +24 dB
to `rect.top()` and -24 dB to `rect.bottom()`, clamped. Sampling shares the node's own
coefficient code through

```rust
// kinewright-media, public from lib.rs
#[must_use] pub fn parametric_eq_magnitude_db(
    effect: &Effect, at: TimeCode, hertz: f64, sample_rate: u32) -> f64;
```

evaluated at a fixed display rate of 48 000 Hz — the app cannot know the device rate — with the
tooltip "Magnitude at 48 kHz."

An expanded `audio_compressor`, `audio_gate`, `audio_ducking`, or `audio_true_peak_limiter` card
carries a gain-reduction bar: `allocate_exact_size(vec2(available_width,
size::MIXER_REDUCTION_METER_HEIGHT), Sense::hover())`, well `SURFACE_ACTIVE` radius 1 (the mixer
meter's well, mixer_ui.rs:526-549), fill **inverted** — growing from the right edge leftward,
width `rect.width() * (reduction_db / MIXER_REDUCTION_METER_RANGE_DB).clamp(0.0, 1.0)` with
`MIXER_REDUCTION_METER_RANGE_DB = 24.0` — in `STATUS_WARNING`, a functional status colour
(DESIGN.md:22-23), never accent, with the same `-{x:.1} dB` readout the collapsed header shows.
Levels come from `MixPeaks.gain_reduction` keyed by `(chain, effect.id)`, decayed by
`MixerMeterLevels` at the same 0.9 per second and raised only while playing (mixer_ui.rs:116-148);
an absent key reads 0.

### 6.8 Insertion, routing gates, and live routing (R23, A6, A11)

`+ Effect` offers exactly six entries, in this order: **Gain** (`audio_gain`), **Parametric EQ**
(`audio_parametric_eq`), **Compressor** (`audio_compressor`), **Gate** (`audio_gate`), **Ducking**
(`audio_ducking`), **True-peak limiter** (`audio_true_peak_limiter`). The menu is filtered by a
**new** predicate:

```rust
/// Whether the mixer's `+ Effect` menu offers one audio effect (AU2 §6.8).
fn is_audio_effect_insertable(name: &str) -> bool;   // the six above; false for audio_eq, audio_limiter
```

`is_effect_insertable` (inspector_ui.rs:3941-3945) is **unchanged**: it excludes audio effects,
which is correct, because `AddEffect` on a clip returns `OpError::AudioEffectOnClip`
(operation.rs:2560-2563). Widening it would surface audio nodes in the clip inspector where
every one is rejected.

**Insertion values and their consequences (A6, N5/Q2).** An insertion appends an `Effect` with
`id = max(chain effect ids) + 1` (N1), the descriptor name, **every parameter written explicitly
at its descriptor neutral**, and no keyframes. The menu behaves uniformly and never diverges from
the descriptor table. Two of the six are therefore **not** identities on insertion, and the UI
says so in the menu entry's tooltip:

- **Ducking** inserts at `threshold_tenth_db = -300`, `reduction_tenth_db = 120`
  (effect.rs:1719-1750) — a working duck of 12 dB whenever the sidechain exceeds -30 dBFS.
- **True-peak limiter** inserts at `ceiling_tenth_db = 0`, `true_peak = 1`,
  `lookahead_milliseconds = 5`. A 0 dBFS *true-peak* ceiling limits ordinary near-full-scale
  material whose *sample* peak is at or below 0 dBFS — that is what a true-peak limiter is for —
  and, because it declares 5 ms of lookahead, **inserting it stops and re-cues playback once**
  (§5.8, §6.10).

Gain, Parametric EQ, Compressor, and Gate insert as exact identities and play through.

Gating: Ducking is `add_enabled(false)` on the master and on a bus with no sidechain track
(tooltip "Ducking needs at least one sidechain track"); an entry is `add_enabled(false)` when
inserting it would push the chain past the 20 ms budget (tooltip "This chain already uses its
20 ms lookahead budget").

**Routing gates.** A routing or sidechain checkbox whose toggle would produce a rejected
operation is disabled with the reason rather than allowed to fail: unchecking the last track of a
bus (`InvalidAudioBus`, operation.rs:3947-3949 — "a bus needs at least one track; remove the bus
instead"), and unchecking the last sidechain of a bus carrying `audio_ducking`
(`AudioBusDuckingWithoutSidechain`, operation.rs:3990-3992 — "this bus's ducking node needs a
sidechain track"). A track already on another bus is shown checked-disabled with that bus's name.

**Live routing (A11).** The predicate's signature changes, because it now needs both documents:

```rust
/// AU2 §6.8: whether a document change is a live mix edit and nothing else.
pub(crate) fn is_live_audio_mix_change(
    journal_command: Option<&JournalCommand>,
    old: &Document,
    new: &Document,
) -> bool;
```

True iff every operation of a non-empty `Do`/`DoBatch`/`DoBatchCoalesced` is one of `SetTrackMix
| UpsertAudioBus | RemoveAudioBus | SetAudioMaster | SetPanLaw` **and**
`old.audio_mix.lookahead_milliseconds() == new.audio_mix.lookahead_milliseconds()`. `None`,
`Undo`, and `Redo` stay false.

Today the call site is `let live_track_mix = is_live_track_mix_change(journal_command.as_ref());`
at **app.rs:1056**, and `previous_document` is not fetched until **app.rs:1057**: those two lines
are **reordered** so the old document is available to the predicate. The live branch's comment at
**app.rs:1158-1161** ("The document differs only in `audio_mix.tracks`, which no video path
reads") is rewritten to "The document differs only in `audio_mix`, which no video path reads, and
declares the same processing latency, so the running processor can be retargeted in place."
`RemoveAudioBus` is safe on the live path: `audio_mix` is read only by render.rs:623,
mixer_ui.rs:229, the media processor, and core validation.

`operation_status` (app.rs:1686+) gains:

- `SetAudioMaster { master } => format!("Set master mix (gain {:+.1} dB, {} effects)",
  f64::from(master.gain_tenth_db) / 10.0, master.effects.len())`
- `SetPanLaw { law } => format!("Set pan law to {}", match law { PanLaw::Balance => "balance",
  PanLaw::ConstantPower => "constant power" })`

**Keys: none new.** `KEYMAP` (keys.rs:73) stays at 21 entries and its length assertion at
keys.rs:445 is unchanged; `Ctrl+Shift+M` still opens the Mixer, and the chain pane is reached only
from an `Edit` toggle.

### 6.9 Tokens and DESIGN.md

New `theme.rs` `size` tokens beside the AU1 mixer tokens (theme.rs:86-91):

```rust
/// Width of the mixer chain pane beside the strips (AU2 §6.6).
pub const MIXER_CHAIN_PANE_WIDTH: f32 = 400.0;
/// Height of the EQ magnitude well on an expanded parametric EQ card (AU2 §6.7).
pub const MIXER_EQ_CURVE_HEIGHT: f32 = 96.0;
/// Height of a gain-reduction bar on an expanded dynamics card (AU2 §6.7).
pub const MIXER_REDUCTION_METER_HEIGHT: f32 = 6.0;
```

`docs/DESIGN.md` Mixer section: the read-only paragraph (:348-352) is replaced by

> A bus strip reads: name, member tracks, a node count with the full chain as its tooltip, one
> row carrying the meter pair and the vertical gain fader with its dB readout, and an `Edit`
> toggle. The master strip is the label `MASTER`, that same meter-and-fader row fed by the
> post-limiter master peaks, a node count, and its own `Edit` toggle. Each track strip carries a
> `+ Bus` button, disabled with its reason when the track has no audio or is already routed.
> Meters read zero whenever nothing is playing.
>
> `Edit` opens the chain pane beside the strips, inside the same dock — the Mixer adds no
> floating surface. The pane carries the chain's routing and sidechain checkboxes, its effects as
> cards, a `+ Effect` menu, and, on the master, the pan-law choice. One card is expanded at a
> time; a collapsed card is a single row with the effect's display name, a `Bypass` checkbox, its
> gain-reduction readout, move-up and move-down buttons, and a `Remove` button in status-danger
> text. An expanded card adds a wrapped row of controls labelled in their own units (dB, Hz, ms,
> ratio, Q), a read-only magnitude well on a parametric EQ, and a gain-reduction bar on a
> dynamics node. The gain-reduction bar is the one meter in the product that fills from the
> right, because it shows how far a signal has been pushed down; it uses status-warning and never
> accent.

DESIGN.md:328-333's "strip 72 wide fits 240 tall" is edited to match whatever §6.5 measures.

### 6.10 Known limits

- **Live non-gain parameter steps are unsmoothed.** Only bus gain, master gain, and the track
  stage ramp. Dragging an EQ frequency, a threshold, or a ratio steps the coefficient on the next
  sample frame, so a fast sweep zippers. Automation on those parameters has always stepped at
  project-frame resolution (M33) and still does.
- **Sidechains tap pre-delay and lead by up to `L_bus`.** That pre-empts the duck rather than
  lagging it, which is the useful direction; the figure is 0 ms for every document with no
  lookahead node and at most 20 ms.
- **Per-node gain-reduction meters are not latency-aligned.** A track meter reads its post-stage
  peak, a bus meter its post-chain peak, a node's reduction the chunk the processor is producing,
  and the master its post-clamp peak, so bars can be up to the graph latency apart on screen. The
  0.9-per-second decay and the roughly one-second ring fill dominate that anyway.
- **A live edit that changes any chain's lookahead stops playback once.** `L` is derived from the
  document, so inserting a true-peak limiter (which declares 5 ms at its neutral), removing one,
  or retuning a lookahead takes the stop-and-re-cue path (§5.8). Every other bus and master edit
  plays through.
- **Inserting Ducking or a true-peak limiter changes the sound immediately.** Their descriptor
  neutrals are a working 12 dB duck and a 0 dBFS true-peak ceiling; the other four offered nodes
  insert as exact identities (§6.8).
- **A true-peak limiter is an identity only for material whose *true* peak is at or below the
  ceiling.** A sample peak at or below the ceiling is not sufficient: inter-sample peaks routinely
  exceed it, which is the reason the node exists. `audio_limiter`'s neutral is already `-10`, and
  `audio_ducking`'s is a working duck, so "every audio neutral is an identity" was never true
  (§2.1).
- **The true-peak detector is not BS.1770-4 conformant.** It has the Annex 2 structure with this
  contract's own Blackman windowed-sinc coefficients and reads within about 0.3 dB of a 16x
  reference; conformance of the delivered file needs the ITU coefficient table (AU3, OPEN-4).
  (AU3 §3.6 resolved this by pinning the measurement true-peak meter against EBU Tech 3341's
  tolerance cases rather than a coefficient table.)
- **Degenerate rates.** Below about 7 kHz a 1 ms lookahead gives fewer than
  `TRUE_PEAK_GROUP_DELAY_FRAMES + 1` sample frames, so the limiter falls back to sample-peak
  detection; below 1 kHz `stage_latency_frames` truncates a 1 ms chain to zero frames and the
  stage applies no delay at all (§3.5, §3.6). Both occur only at the rates the unit tests use.
- **A structural chain edit resets that chain's state.** Inserting, removing, or reordering a node
  during playback restarts its filters and envelopes, audible as a momentary discontinuity on
  that chain only. Creating a bus is seamless (§5.8).
- **Undo and redo stop playback.** The AU1 limit stands.
- **A one-channel device applies no pan under either law**, so under `ConstantPower` a
  hard-panned track reads `+3.01 dB` on a mono device against its stereo per-channel level.
  **Channels beyond the first two take the unpanned gain**, so under `ConstantPower` they sit
  `3.01 dB` above the stereo pair at centre. Playback/export parity is claimed at two channels
  only, as it has been since M12.
- **Frequency and Q keyframes interpolate linearly in their integer unit**, not logarithmically,
  which is musically wrong across a wide sweep. A consequence of "integer document controls with
  stable units" (ROADMAP:630-633); use closer keyframes.
- **A filter's decay tail below about -600 dBFS is truncated** by the small-value squelch (§3.2),
  so a section's tail is not bit-reversible.
- **An expanded chain card can be taller than the Mixer dock** and scrolls inside the pane; only
  one card is expanded at a time to keep that rare (§6.7).
- **`measure_mix_levels` and `mix_spectrum` hold stems in memory.** Roughly `(2 x tracks +
  buses) x 48 000 x 2 x 4` bytes per second of `range.end`, about 1.8 GB for four tracks over ten
  minutes (AU1 §0). A per-chunk accumulator is the AU3 remedy. (AU3 §3.8 delivered it for
  `measure_mix_levels` and audio QC through `MixObserver`; `mix_spectrum` remains the whole-stem
  path.)
- **A playback device not running at 48 kHz gets different coefficients from export.** Every
  filter is designed from the runtime rate, so a 44.1 kHz device's bilinear warping puts a band a
  fraction of a decibel from where export puts it, and a 20 kHz band is clamped to `0.45 x rate`.
- **The five spectrum bands below 63 Hz are window-limited** and flagged as such; their reading is
  dominated by leakage from their neighbours.
- **Constant-power pan changes a neutral document's output** by design. It is off by default, so
  AU1's bit-identity promise holds for every existing project.

## 7. Acceptance checklists

### Part A (A1–A19)

Discharges exit-gate clauses 1 and 2 verbatim from ROADMAP-AND-WORKFLOWS.md:646 — *"Filter
magnitude response matches the analytic transfer function at pinned frequencies; gain reduction
and ceiling are measured on synthetic material"* — and the **bus half** of clause 3,
*"playback/export parity through every node"*, which B10 closes (§1.2).

A1. Every new descriptor parameter's `min`, `max`, `neutral`, and `EffectUniform` variant are
    exactly the tables of §2.1; there are **40 new rows and 33 new variants**, with one shared
    `AUDIO_BYPASS_DESCRIPTOR` / `EffectUniform::AudioBypass`; `is_audio_effect` accepts exactly
    eight names — core `tests/au2_core.rs`.
A2. The 33 new uniforms compile into the compositor's exhaustive ignore arm
    (compositor.rs:2658-2672) — **enforced by the compiler, not a test**; the three new
    descriptors stay unclassified for colour staging, so `contracts.rs:2519-2529` and
    `compositor.rs:5399-5426` are unchanged and both still pass — core, media.
A3. Each descriptor's runtime neutral fallback equals its descriptor neutral (empty `parameters`
    map is bit-identical to all-neutral) — media `audio.rs`.
A4. A `Linear` curve on `bypass`, `detector`, or `true_peak` is rejected with
    `NonHoldKeyframeParameter`, and any curve on `lookahead_milliseconds` with
    `InvalidEffectAutomation`, from the operation and again from `Document::validate` on a
    hand-edited document — core `tests/contracts.rs` and `tests/au2_core.rs`.
A5. **Filter magnitude response matches the analytic transfer function at pinned frequencies**
    (exit-gate clause 1): peaking `+6.00 dB` ±0.01 at `f0`; shelves at DC, `f0`, and Nyquist
    ±0.02; high-pass `-3.0103 dB` ±0.01 at `fc` for `fc` ∈ {100, 1 000, 8 000} Hz and
    `-12.32 dB` ±0.05 at 500 Hz for `fc = 1 000 Hz` at 48 kHz — media `audio.rs`.
A6. A neutral `audio_parametric_eq` is bit-identical to its input at **node level**, and a
    bypassed node of every kind is bit-identical to its delayed input — media `audio.rs`.
A7. A chain declaring 20 ms of lookahead is accepted and one declaring 21 ms is rejected with
    `AudioBusLookaheadExceeded`, in the operation and at load — core `tests/au2_core.rs`.
A8. Compressor static-curve pins: below-threshold identity, 12 dB over at 4:1 giving 3 dB over,
    knee midpoint at `(1 - 1/R) * W/8` — media `audio.rs`.
A9. **R24:** an existing-shaped compressor document (`threshold -120`, `ratio 400`, no new
    parameters) is bit-identical to a stored expectation — media `audio.rs`.
A10. RMS versus peak detection on a tone burst; gate range floor, 1:1 identity, zero-range
     identity, exact hold count — media `audio.rs`.
A11. **Ceiling is measured on synthetic material** (exit-gate clause 2), verified with an
     **independent 16x windowed-sinc interpolator written in the test**: the ceiling is never
     exceeded on an over-level tone under both detectors, **nor on an isolated full-scale impulse
     at `true_peak = 1` with `Lh` at its minimum**; an under-ceiling signal is bit-exact at
     48 kHz **and** at rate 1 000 with a 1 ms release; the release constant is within 5 %; the
     `fs/4` pin reads sample peak `-3.010 dBFS` ±0.01 and detector `0 dBFS` **±0.3** — media
     `audio.rs`.
A12. **Gain reduction is measured on synthetic material** (exit-gate clause 2):
     `MixPeaks.gain_reduction` reports a positive dB figure for every compressor, ducking, gate,
     and limiter node, keyed by `(chain, effect id)`, and nothing for the other kinds — media
     `audio.rs`.
A13. **R25:** every name for which `is_audio_effect` is true changes a full-scale tone by more
     than 1e-3 at a non-neutral setting — media `audio.rs`.
A14. `L == 0` for every pre-AU2 document: `mix_chunk` output, the four existing unit-test
     fixtures, and export bytes are unchanged — media `audio.rs`, `export.rs`.
A15. With a lookahead node: an impulse at input frame 0 emerges at output frame
     `graph_latency_frames(&lookahead, rate)`, asserted at **48 000 Hz and 44 100 Hz**; export
     puts an impulse at project frame `k` at exported frame `k`; the tail survives the flush;
     `open` at frame 0, at frame 5, and **at the last frame** each emit their first output frame
     from the matching input frame and the cursor lands where AU1 says — media `audio.rs`,
     `export.rs`.
A16. **A2:** `measure_mix_levels` over a one-frame range containing one impulse reports it in the
     track stem, the bus stem, and the master, **and all three report the same `sample_frames`**,
     on a document whose bus carries a lookahead limiter — media `export.rs`.
A17. **Playback/export parity through every node — bus half** (exit-gate clause 3, closed by
     B10): `assert_playback_matches_export` on `parity_document_with_full_chain` (nine windows at
     1e-6 plus the frame-5 seek), plus a legacy-node variant; `parity_document` and
     `parity_document_with_track_mix` unchanged — media `audio.rs`.
A18. The `upsert_audio_bus` description names the four preferred nodes, "do not add them to a new
     chain", "20 ms", and the hold-only rule, and never says "ITU";
     `audio_parametric_eq_pattern_documentation()` replaces the twelve band rows; the tool count
     stays 126 and `INSPECTOR_TOOL_NAMES` stays 76; **`input_schema_bytes` is asserted unchanged
     at 1 186 449** and only `description_bytes` is regenerated; the served triple
     7 / 5 660 / 3 510 / 998 is asserted byte-identical — agent `schema.rs`, `server.rs`,
     `tests/mcp_server.rs`.
A19. **A15 additions.** The `0.45 * fs` frequency clamp and the small-value squelch are each
     pinned (a 20 kHz band at 44.1 kHz stays stable; a settled section's state reaches exactly
     zero); per-project-frame evaluation is asserted **bit-identical to a per-sample-frame
     reference implementation** over a keyframed parameter; and a bench or timed test records the
     per-second cost of a worst-case chain (a 10 ms true-peak limiter plus a 10 ms RMS lookahead
     compressor at 48 kHz stereo) so `LIVE_FILL_MILLISECONDS` need not be revisited — media
     `audio.rs`, `spectrum.rs`.

### Part B (B1–B21)

Closes exit-gate clause 3 (B10) and discharges AU1's standing clause *"serialized defaults are
omitted and hand-edited values are rejected on load"* (B2, B4) plus the AU2 row's remaining
**deliverables** — *"bus and master control editing in the mixer; constant-power pan as a second
law; spectrum evidence for the agent"* (§1.2).

B1. `SetAudioMaster` and `SetPanLaw` join the every-variant JSON round trip after
    `RemoveAudioBus`, and `UpsertAudioBus` carries `gain_tenth_db` — core `tests/contracts.rs`.
B2. A neutral `AudioMaster` and `PanLaw::Balance` are omitted; a document only ever set neutral
    serializes byte-identically to one never touched; `AudioMix::is_empty` is still `const` —
    core `tests/au2_core.rs`.
B3. Bus and master gain bounds accepted at ±limits and rejected outside with
    `AudioBusGainOutOfRange` / `AudioMasterGainOutOfRange`; every rejection leaves the document
    equal to `before` — core `tests/au2_core.rs`.
B4. Master chain rules: a visual effect, a duplicate master effect id
    (`DuplicateAudioMasterEffect`), `audio_ducking`, a keyframe at or past `duration`, an
    out-of-range parameter, and a 21 ms chain are each rejected with the named `OpError`, from
    the operation and again from `Document::validate` — core `tests/au2_core.rs`.
B5. `AudioMix::next_bus_id` returns `max + 1` and agrees with `normalization_context`'s former
    inline allocator; no document-wide effect-id helper exists; `color_status::next_effect_id` is
    unchanged — core `tests/au2_core.rs`, agent `server.rs`.
B6. Ten `DoBatchCoalesced` with key `audio_bus:1#1` and increasing thresholds collapse to one
    undo entry; one `Undo` yields `initial` — core `tests/au2_core.rs`.
B7. `needs_seek_preroll` is true for a bus-free document with one master effect
    (`decode_from == TimeCode::ZERO` at frame 5) and still false for a track-mix-only document —
    media `audio.rs`.
B8. **Constant-power pan as a second law** (deliverable): `L^2 + R^2 == 1` within 1e-6 at every
    integer pan; the three cardinal positions exact under `==`; centre `-3.0103 dB` ±0.001;
    `Balance` bit-identical to AU1; a mono processor returns `[gain, gain]` and a four-channel
    processor leaves channels 2 and 3 at `gain`, under both laws — media `audio.rs`.
B9. Bus and master gain ramp over 240 sample frames at 48 kHz and settle exactly; a fresh
    processor never ramps — media `audio.rs`.
B10. **Playback/export parity through every node — closed** (exit-gate clause 3):
     `assert_playback_matches_export` on `parity_document_with_full_chain` **extended with the
     master chain** (one gain plus one true-peak limiter), with `parity_document` untouched —
     media `audio.rs`.
B11. The live diff preserves state on a parameter-only edit and rebuilds on an insertion, a
     removal, and a reorder; a bypass flip retargets in place; **a parameter retarget is audible
     on the next sample frame, not the next project frame** (A13, asserted at 10 fps); creating a
     bus mid-play leaves no gap — media `audio.rs`.
B12. **R8:** `MixMeters::matches_document` is true after ten consecutive live updates that change
     only a gain (`Arc::ptr_eq` on the installed table) and false after a structural edit — media
     `audio.rs`, `engine.rs`.
B13. **Spectrum evidence for the agent** (deliverable): a 1 kHz tone peaks in the 1 000 Hz band
     and is at least 40 dB down two octaves away; the same at 80 Hz; a full-scale sine inside one
     band reads `0.00 dBFS` within 5 hundredths (which fails by 42 dB without the `1/N`, A5);
     `window_limited` is true for exactly the 20, 25, 31.5, 40, and 50 Hz bands; a 400 ms range
     returns `MixSpectrumRangeTooShort`; the in-house FFT matches a direct DFT at `N = 64` within
     1e-9 — media `spectrum.rs`, `export.rs`.
B14. Ordered tool-name list and positions; `set_audio_master` and `set_pan_law` annotated
     `idempotentHint: true`, `destructiveHint: false`; `upsert_audio_bus` annotated idempotent;
     `set_pan_law`'s description contains "constant_power", "-3.01 dB", and the channel sentence;
     **`set_track_mix`'s description no longer asserts a law and names `set_pan_law`, and
     `set_track_mix_schema_documents_ranges_and_the_balance_law` (schema.rs:896) is updated with
     it** — agent `schema.rs`.
B15. Registry counts 52 / 77 / 129; served triple byte-identical; internal figures regenerated;
     every site in §6.4 moved — agent `server.rs`, `tests/mcp_server.rs`.
B16. Compact rendering of `gain=`, `audio_master`, and `audio_pan_law=`, with the existing golden
     byte-unchanged — agent `render.rs`.
B17. `au2_bus_and_master_editing_round_trips_through_edit_plans_and_state` and
     `au2_get_audio_spectrum_measures_the_real_mix`, placed after the AU1 pair
     (tests/mcp_server.rs:2517-2603 and :2605-2814) and mirroring their shape — agent
     `tests/mcp_server.rs`.
B18. **Bus and master control editing in the mixer** (deliverable), fold pin: through
     `mixer_body`, a frame in which a bus's **strip fader** and one of its **pane controls** both
     change produces **exactly one** `UpsertAudioBus` carrying both changes, with coalesce key
     `audio_bus:{id}`; a frame touching a bus and the master produces exactly one of each; no
     control pushes an operation directly — app `mixer_ui.rs` (`MixerHarness` with its new
     `selection` field).
B19. **R10, A9:** `measure_bus_strip`, `measure_master_strip`, and `measure_chain_card` exist; a
     bus with six nodes, a sidechain, and a non-zero gain fits `BUDGET = 240.0`; a collapsed card
     is at most `ICON_BUTTON` tall; the strip row paints `MASTER`, each track caption, each bus
     name, the node count, and `+ Bus`, and no longer paints "Bus controls arrive with AU2" — app
     `mixer_ui.rs`.
B20. Builders and gates: the bus fader, master fader, a card parameter drag, a bypass flip, a
     move, a remove, an insertion, a routing checkbox, `+ Bus`, and the pan-law choice each
     produce the exact folded operation; `+ Bus` is disabled with "no audio" / "already routed";
     the `+ Effect` menu offers exactly six entries with the eight display names of §6.7, and
     disables Ducking without a sidechain and the limiter over budget; the last-track and
     last-sidechain checkboxes are disabled with their reasons; `is_audio_effect_insertable` is
     new and `is_effect_insertable` is unchanged; `is_live_audio_mix_change`'s truth table
     including **a negative case where every operation is eligible but only the lookahead
     differs**; `operation_status`' two new strings — app `mixer_ui.rs`, `inspector_ui.rs`,
     `app.rs`.
B21. **A15 additions.** The gain-reduction bar's filled rect is anchored to the right edge, grows
     leftward as reduction rises, and reads its level from `MixPeaks.gain_reduction` by
     `(chain, effect id)`; the EQ well's 96 sampled points match `parametric_eq_magnitude_db` at
     48 kHz within 1e-6 and its log-frequency mapping puts 1 kHz at the expected x; the three new
     `theme.rs` tokens resolve; and `DESIGN.md`'s Mixer section contains the new sentences and no
     longer contains "read-only" — app `mixer_ui.rs`, `theme.rs`, docs.

Exit gate for each part: its checklist green in `cargo test --workspace` locally on Linux,
`cargo fmt --check` and `cargo clippy --workspace --all-targets -- -D warnings` clean, both CI
operating systems green after push, and both review passes recorded. Part B additionally needs a
hands-on Omarchy/Windows smoke of live bus and master editing on a real device, recorded by Riel.
Part A lands as "feat: complete AU2a — EQ and dynamics nodes"; Part B as "feat: complete AU2b —
bus and master control".

## 8. Files

**Part A.** Core: `effect.rs` (descriptors, `AUDIO_BYPASS_DESCRIPTOR`, 33 uniforms),
`operation.rs` (`is_hold_only_parameter` audio branch, `is_static_audio_parameter`,
`chain_lookahead_milliseconds`, `ChainLookahead`, `AudioBusLookaheadExceeded`,
`validate_audio_bus`), `model.rs` (`AudioMix::lookahead_milliseconds`,
`Effect::static_integer_parameter`), `media.rs` (`AudioChain`, `MixPeaks.gain_reduction`),
`lib.rs`, `tests/contracts.rs`, new `tests/au2_core.rs`. Media: `audio.rs` (nodes,
`stage_latency_frames`, `graph_latency_frames`, meters, `MixMeters::matches_document`),
`export.rs` (flush and head-and-tail per-family trim), `compositor.rs` (ignore arm), `lib.rs`,
tests. Agent: `schema.rs` (prose, pattern documentation), `server.rs` /
`tests/mcp_server.rs` (byte figures, served triple). Docs: this file, `CHANGELOG.md`,
`MEDIA-POLICY.md`, `M33-PARAMETRIC-DEPTH-VERIFICATION.md`,
`M36-AGENT-RUNTIME-EFFICIENCY.md`.

**Part B.** Core: `model.rs` (`AudioBus.gain_tenth_db`, `AudioMaster`, `PanLaw`, `AudioMix`
fields and accessors, gain constants), `operation.rs` (two variants, `validate_audio_master`,
seven `OpError`s, the `operation.rs:100` doc comment), `media.rs` (`MixSpectrum*`,
`MediaError::MixSpectrumRangeTooShort`, the `Analysis` method), `lib.rs`, `tests/contracts.rs`,
`tests/au2_core.rs`. Media: `audio.rs` (master chain, gains, pan law, `update_audio_mix` and its
two renamed forwarders, preroll), `export.rs` (`measure_mix_spectrum`), `engine.rs` (worker
update, meter install), new `spectrum.rs`, tests. Agent: `schema.rs`, `render.rs`, `server.rs`,
`tests/mcp_server.rs`. App: `app.rs` (predicate signature and call-site reorder, status),
`theme.rs`, `mixer_ui.rs` (`mixer_body`, strips, pane, cards, `MixerChainEdits`, harness),
`inspector_ui.rs` (`effect_display_name`, visibility), `color_qc_ui.rs` (delegate). Docs: this
file, `README.md`, `CHANGELOG.md`, `ROADMAP-AND-WORKFLOWS.md`, `DESIGN.md`,
`M36-AGENT-RUNTIME-EFFICIENCY.md`.
