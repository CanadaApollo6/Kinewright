# AU3 Loudness and delivery

Status: draft for critic review (2026-09-08, revision 3). Built from the AU3 design brief (D1–D10)
as amended by its **Addendum** (rulings on `critic-brief.md` F1–F23, Q1–Q8), by
`critic-contract.md` F1–F17 / Q1–Q3, and by `orchestrator-notes.md` N1–N4, against main 5dcf0d2.
Where a later ruling and an earlier one conflict, the later wins; N4 overrides `critic-contract.md`
wherever they differ. The numbers in this document are the contract; implementation follows them or
amends this file first.

AU3 is the third slice of the audio programme (ROADMAP-AND-WORKFLOWS.md:647). It lands as **one
contract in two parts, two commits**: **Part A — Measurement** (the streaming loudness engine, the
five measurements, the report types, the live meter, `LoudnessTarget`, `Analysis::audio_qc` and
`get_audio_qc`, the QA mute/solo rule, the Mixer loudness section; no export behaviour change) and
**Part B — Delivery** (normalization as an export step, decoded audio verification of the written
file, the planner on the true-peak limiter, the export dialog row and audio verification block).
Part A lands first. Each part runs the full workspace gate and both review passes.

## 0. Errata

The implementation records each deliberate deviation from the text below here, as AU2 §0 does. Two
figures are regenerated as the code lands: Part A's registry triple after `get_audio_qc` and the two
rewritten descriptions, and Part B's after the `queue_export` argument and the three rewritten
descriptions.

**Resolved before implementation (N3, on revision 2's OPEN-1..6):**

- **R1 (was OPEN-1). Conformance tones carry a 10 ms raised-cosine fade in and out** as part of the
  fixture definition: an unfaded 0.5-amplitude fs/6 60° tone reads −5.611 dBTP from its onset
  step against −6.02 +0.2; faded it reads −6.0206 (§3.6).
- **R2 (was OPEN-2). The honest memory figure is ~0.9 GB** for four tracks over ten minutes: the
  observer removes the stems (export.rs:907-914, :934-939) and the decoded per-track buffers
  (export.rs:881-883) remain; "O(blocks)" applies to the measurement state only (§3.8, §6.9).
- **R3 (was OPEN-3). The 997 Hz calibration reads 0 / 0 / −2 hundredths at 44.1 / 48 / 96 kHz**
  (K gain +0.6938 / +0.6910 / +0.6737 dB), pinned exactly (§3.2); F1's "0 ±1 at all three rates"
  is off by one at 96 kHz.
- **R4 (was OPEN-4). Case numbers.** The critic confirmed Tech 3341 v3 cases 1–5, 9, 12, 15–18 and
  Tech 3342 v3 cases 1–4 with their real durations and levels; the implementer records them in test
  doc comments as "per EBU Tech 3341 v3 / Tech 3342 v3".
- **R5 (was OPEN-5). `max(phase estimate, |x|)` is structural.** With `x_n = (n − 128)/8` and
  per-phase normalisation, phase 0 has a single non-zero tap (`k = 16`, exactly 1.0) so the
  oversampler's phase-0 output is the input delayed 16 frames; no separate maximum is coded. Per
  N4 the off-centre phase-0 taps are asserted `< 1e-15`, not equal to zero (§3.6).
- **R6 (was OPEN-6). `ExportJobRecord.audio_verification_unavailable_reason`** follows CC6 E31's
  sole-carrier rule (§6.2).

**Resolved by N4 (on `critic-contract.md`):** Q1 the live meter publishes **by audible position**
(§3.9); Q2 **two hot lanes** plus the failing direction, the sine-plus-noise lane labelled "no
limiting needed" (§5.8); Q3 a range shorter than one gating block is **refused typed**
(`MediaError::MixLoudnessRangeTooShort`) on `mix_levels` and `audio_qc`, so `integrated == None`
means silent and only silent, and the normalization skip reason for a short programme is distinct
(§2.5, §3.3, §5.6); F4 the literal sites are enumerated and `cc6_core.rs` is dropped (§5.1, §8); F5
the `pub(crate)` entries are named (§3.9, §5.6, §5.7). No OPEN note remains.

**Implementation errata (Part A core, 2026-09-09):**

- **E1. Wire spelling of the profile.** §7 A1's example `"profile":"youtube_1080p"` is wrong:
  `DeliveryProfile` has serialized as `youtube1080p` (`rename_all = "snake_case"` on the variant
  name) since CC6, and `youtube_1080p` is the `as_str()` spelling that cc6_core.rs pins separately.
  Tests follow the real wire; the rename is unchanged.
- **E2. `audio_qc_exceptions` takes `&AudioQcMeasurements`**: `master`,
  `channel_balance_lu_hundredths`, `clipping`, the two `*_silence_frames`, the two
  `*_silence_milliseconds` (so the 1 000 ms rule uses media's window count rather than a frame-rate
  rounding), and `target`.
- **E3. `audio_clipping` is one exception per clipped channel** (`field:
  clipping.left.clipped_runs` / `clipping.right.clipped_runs`, `observed` = that channel's run
  count); the §2.4 table's per-channel field spelling is what makes the `field asc` tiebreak
  meaningful.
- **E4. `observed` strings** where the table gave none: `audio_silent` → `"none"`; the two silence
  infos → `"{frames} frames ({ms} ms)"`; target and balance entries → the integer.
- **E5. The "Δ `None` with `J ≠ ∅` and one side zero" case is derived in core** from
  `integrated.is_some() && channels >= 2 && balance.is_none()`; media passes no extra flag, and a
  mono master with `None` raises nothing.
- **E6. `LOUDNESS_GATING_BLOCK_FRAMES` lives in media**, as §8's core list implies; core mentions
  19 200 only in `MixLoudnessRangeTooShort`'s doc comment.
- **E7. `audio_qc_technical_pass(&[AudioQcException]) -> bool`** is a small addition beyond the
  named functions so media does not re-derive the "no Error" rule.
- **E8.** The media.rs unit test is renamed
  `working_proof_delivery_verification_and_audio_qc_default_to_not_implemented` to cover the
  `audio_qc` default (A4's media.rs half).

**Implementation errata (Part A app, 2026-09-09):**

- **E9. `Reset` shares the readout row.** §4.4's list gives `Reset` its own item; a five-row stack
  measured 126 px against the 80 px budget with the theme's 26 px `interact_size`. The section
  scopes `item_spacing.y = HALF` and `interact_size.y = ICON_SM` (the pane's card-row precedent),
  paints each bar row one text row tall, and right-aligns `Reset` on the readout row: 74 px
  measured, the master strip 196 → 210 px against its 240 px budget. The budget is the normative
  constraint; the row order is otherwise as written.
- **E10. ASCII minus in readouts.** `-18.3` / `I -16.0` follow `export_ui::decibels` and
  `reduction_readout`, the app's existing signed-hundredths renderers, rather than §4.4's U+2212;
  `None` renders as U+2014 `—` as written, which never reads like a hyphen.
- **E11. The reset flag's carriers.** `chain_pane` returns `bool` and `mixer_body` returns
  `MixerFrame { selection, reset_loudness }` (not `Option<MixerSelection>`) so the click reaches
  `mixer_panel`, which calls `Playback::reset_loudness()` (idempotent: an egui discarded pass
  may run it twice at the same audible position, which is one restart) and emits no `Operation`. §4.4 named
  only `loudness_section(...) -> bool`.
- **E12. `loudness_bar_color(Option<i32>, LoudnessTarget) -> Option<Color32>`**: "`None` draws no
  fill" is a third outcome, so the pure function returns `None` rather than a sentinel colour.
- **E13. Polling lives in `mixer_ui::mixer_panel`, not `app.rs`.** `mixer_telemetry` reads the
  snapshot every frame the Mixer tab is visible and expanded (peaks only while playing, loudness
  always, per F15); a hidden Mixer costs nothing. §8's Part A `app.rs` entry is therefore unused.
- **E14. A19's dsp.rs pin lives in the media crate.** The app docs test pins DESIGN.md,
  CHANGELOG, MEDIA-POLICY, the AU1 and AU2 pointer sentences, and the M36 Part A rows; the
  `dsp.rs:307-312` amendment is media's file and is pinned by a media `include_str!` test
  (`the_true_peak_detector_note_names_the_au3_meter`), not by the app's.
- **E15. The master strip's `I …` line carries the full `loudness_readout` as its tooltip**, the
  idiom the strip already uses for its node count (DESIGN.md "a node count with the full chain as
  its tooltip"); §4.5 did not name a tooltip.

**Implementation errata (Part A agent, 2026-09-09):**

- **E16. The no-target text line.** §4.1's template shows only the with-target shape; without a
  profile the first line reads `target=none±none ceiling=none` (each through
  `render_optional_hundredths`), pinned in both agent tests. `profile=` renders
  `DeliveryProfile::as_str()` (`youtube_1080p`), the `id` `get_delivery_profiles` publishes and the
  spelling §4.1's `profile={id|none}` names; the accepted wire spelling is E1's `youtube1080p`.
  The envelope's assumptions list is keyed off `report.target`, not the argument, so it cannot
  disagree with the report it carries (CC6's rule).

**Implementation errata (Part A media, 2026-09-09):**

- **E17. The live-meter lag bound is eleven sub-blocks, not ten.** §3.9's "≤ 10 blocks" counted the
  1 s fill target alone; the head can also lead the audible position by the over-fill chunk, the
  chunk `fill_ring` holds pending, and up to one sub-block of key quantisation:
  `48 000 + 2 × 1 024 + 4 800` frames. The test asserts that derived bound and fewer than 16 ring
  entries; the measured worst lag is printed (`AU3_LIVE_LAG_WORST_FRAMES`, 52 800 here).
- **E18. `FamilyWindow` trims in the feed (`mix_pass`), not in `LevelsObserver`.** Only `mix_pass`
  knows the latency, the bus stage frames, and `keep_from`, so every observer receives exactly the
  trimmed frames and holds meters only.
- **E19. `fill_ring` and `AudioRuntime::fill` take `&mut LiveLoudnessMeter`** (meter plus the
  16-entry ring, in loudness.rs) rather than a bare `LoudnessMeter`, because a ring entry must be
  recorded the instant a sub-block closes inside the fill loop; the atomics and the
  publish/pause/reset state live in engine.rs (`LiveLoudness`, `WorkerLoudness`).
- **E20. `AudioRuntime::open` no longer runs the initial fill.** The meter's rate and channel count
  are the device's, known only after `open`; the worker calls `WorkerLoudness::begin` and then
  `runtime.fill(meter)` before `play()`, in the same `and_then` chain. Fill-before-play holds.
- **E21. The publish key is `SharedClock::position_samples − frame_to_samples(from)`**, the ring
  frames actually consumed, rather than §3.9's `frame_to_samples(position − reset_frame)`, which
  would round through a frame-quantised `TimeCode`. Pause uses the `TimeCode` as N4 defines
  `paused_at`.
- **E22. `ResetLoudness` while playing keys from the mixer's fed output position**, since the audio
  between the fill head and the loudspeaker is already rendered and cannot be re-metered;
  `loudness()` reads `default()` until the sound reaches it (≤ ~1 s). `set_document` also resets
  the meter (a new programme); the live `UpdateAudioMix` path does not.
- **E23. `truncate_to(frames)` sets `sample_frames = frames`** so the ring keys and the
  clock-derived audible count share an origin; the discarded open sub-block (< 100 ms of heard
  audio) is counted in frames but not integrated, inside A12's ±1 hundredth pin.
- **E24. `mix_audio` bit-identity is asserted in-tree as `NoObserver` == with-stems == clamped
  observer feed within one run**, with the SHA-256 printed (`AU3_MIX_AUDIO_SHA256`) rather than
  pinned across OSes, in CC6's print-the-measurement manner; a cross-OS hash would pin libavcodec,
  not this change.
- **E25. A13's L −20 / R −26 dBFS case also raises `audio_channel_imbalance`** (Δ = 600 > §2.4's
  300); the test asserts the Δ pin and the exception with its `observed`/`allowed`.

**Implementation errata (Part B core, 2026-09-09):**

- **E26. `delivery_audio_exceptions(&AudioLoudness, Option<LoudnessTarget>) -> Vec<AudioQcException>`
  is a core addition.** §5.3 describes the delivery exception list in prose while B3 asks the core
  test to pin `delivery_audio_silent`'s message, so the rule lives in core (E7's precedent): it is
  `loudness_target_exceptions(.., "delivery")` plus the silent Warning, re-sorted. Unlike
  `audio_silent`, it does not suppress the target checks: a silent-gated file whose true peak is
  over the ceiling still raises `delivery_true_peak_over_ceiling`.
- **E27. Sixteen `ExportSettings` literals in ten files, not seventeen in eleven.** §5.1's table
  credits `cc6_fixtures.rs` with a literal; `cc6_delivery_settings` calls
  `DeliveryProfile::export_settings` and needed no change.
- **E28. `delivery_audio_silent`'s message** is "The written file's audio reported no gated
  loudness: it is silent, or shorter than one 400 ms gating block." §5.3 gave only the
  parenthetical.
- **E29. `ExportSettings` is not compared with `assert_eq!` after a round trip**: `ExportCancellation`'s
  `PartialEq` is `Arc::ptr_eq` (CC6 §9.5), so B1 asserts the field and compares with the original
  handle substituted in, as cc6_core.rs does.
- **E30. `DeliveryAudioVerification.target` is a required key** (`null` when absent) rather than a
  skipped optional, as §5.3's struct is written, so an absent target is explicit on the wire for the
  app's "no target" label.
- **E31. `tests/au3_core.rs` carries its own `assert_integer_leaves`**, a copy of contracts.rs's;
  integration test targets are separate binaries and core has no shared test helper crate.

**Implementation errata (Part B agent, 2026-09-09):**

- **E32. `verifying` is true for every post-encode wait.** §6.2 makes the audio measurement
  unconditional, so `set_verifying(state, id, true)` replaces `work.verify`; terminal records still
  carry `verifying: false`, so CC6's pins hold.
- **E33. `get_export_jobs`' audio line renders `target=` as the target's integrated LUFS hundredths
  and `ceiling=` as its true-peak ceiling** (or `none`); §6.4 wrote `target={t|none}` without
  naming the projection.
- **E34. The planner's re-cue warning is in the description's first sentence.** §6.3's text put it
  second, but `get_capability`/`search_capabilities` publish only `first_sentence(description)`, so
  the sentence an agent needs before committing was invisible; the description is one sentence
  with no embedded dot, and the §6.4 byte split is regenerated accordingly (planner +124 B, not
  +120: the fold added four bytes, against a second sentence that reached no agent at all).
- **E35. The success path is `verify_and_settle`** (drift → video → audio → settle, unchanged order)
  with `run_work_item(&WorkItem)`; `export_job_lines(&[ExportJobRecord]) -> String` is a free
  function so the text is unit-testable; `NormalizationContext` derives `Debug` for the refusal
  test.
- **E36. A job cancelled during the audio measurement carries neither `audio_verification` nor a
  reason**, CC6's "nothing to report" shape for an abandoned export, matching the video lane;
  cancel-after-encode carries `EXPORT_CANCELLED_BEFORE_VERIFICATION` in the audio reason as §6.2
  says, and never a stale `audio_report`.

**Implementation errata (Part B app, 2026-09-09):**

- **E37. `audio_verification_lines(v, report, profile: DeliveryProfile)`**, not a bare
  `LoudnessTarget`: §6.6's own no-target line prints `({profile})`, which a target cannot render;
  the target is derived inside via `loudness_target()`, so one table still feeds the row, the
  block, and the Mixer.
- **E38. The Mixer `LOUDNESS` budget is 90 px (measured 86), not E9's 80.** §4.6's Part B suffix
  ("; the export step normalises the file") wraps the muted F21 line to two `MICRO` rows at the
  pane width (+12 px). E9 cut the layout to fit the budget; here the sentence is contract text, so
  the budget moved instead. The pane still fits a 260 px dock and scrolls. §4.4 and A18 are
  amended.
- **E39. `ExportOutcome` is a struct** `{ path, result: Result<ExportReport, MediaError>,
  verification, audio_verification }` and `poll_export` reads `report.audio`;
  `verification_block(ui, verification, audio, report, profile)` gained the audio arguments, so
  its two call sites changed. `ExportAudioVerification { Measured(Box<..>), Unavailable(String) }`.
- **E40. `cancelled_before_verification -> Option<&'static str>`**, and picture and sound share
  `contained_measurement`, so the cancel / refusal / panic rules cannot diverge between the two
  halves; `worker_verification` and `run_export_after_preflight` are generic over the encode's
  payload.
- **E41. `audio_verification_status` checks severity before the target.** Core raises nothing
  without a target, so on real data the two orders agree; an `Error` that ever reached the app
  reads `AUDIO OVER CEILING` rather than hiding behind `AUDIO MEASURED`. `technical_pass` is
  carried and not rendered: the status label is its equivalent (only the ceiling is an Error).
- **E42. B14 is pinned on `export_loudness_target(bool, Option<DeliveryAspect>)` plus a real
  pointer click on `loudness_row`**; `start_export` needs a `KinewrightApp`, which no app test can
  build, and the field assignment is the one expression the compiler ties to that function.
- **E43. `KINEWRIGHT_SCREENSHOT_SHOW=export` opens the dialog on its controls only.** The harness
  has no way to seed a finished `ExportOutcome`, so the `AUDIO` block is painted headless by
  `au3_the_audio_block_measures_when_no_target_was_asked_for` instead. The dialog body scrolls at
  `EXPORT_DIALOG_MAX_BODY_HEIGHT` (420, a viewport, not a content bound; CC6 behaviour); the
  measured body heights are recorded in E44.
- **E44. The verification block shows when either half is present**, and its status line is
  `Exported {path} · {video} · {audio}`. Measured at the 460 px harness width: `Loudness` row 96 px,
  widest `AUDIO` block 196 px; the whole body measures **572 px pre-verification** (no export
  running) and **1 480 px worst case** (non-conforming picture verification + widest `AUDIO` block +
  normalization line), pinned ±1 px by `au3_the_export_dialog_body_measures_past_its_scroll_viewport`,
  which lays the body out inside a real `ScrollArea` at the 420 px viewport on the first frame
  (before the floating scrollbar takes its 6 px, so the running app wraps a few px taller). The body
  has scrolled since CC6; the screenshot lane therefore shows the controls down to the `Loudness`
  row, with the muted note clipped at the scroll edge and `Export MP4` one scroll below at any window
  size (capture: target/review/au3/au3-export-dialog.png). To make the body
  measurable, it is extracted verbatim into `export_dialog_body(ui, &mut ExportDialog,
  &ExportDialogBodyContext) -> Vec<ExportDialogRequest>`; the six click flags became that enum,
  re-applied by `show_export_dialog` in the original order. The LRA line carries no verdict because
  every shipped profile's range maximum is `None`.

**Implementation errata (Part B media, 2026-09-09):**

- **E45. §5.6's steps 1 and 2 are exchanged**: the measurement precedes the length check, the
  only order under which step 1's own `after = before` is defined, and a short-programme skip
  therefore carries real `true_peak`/`sample_peak` readings. Skip precedence is unchanged (a
  sub-400 ms programme has `integrated == None` anyway, so the short reason still wins) and the
  three reason strings — `"shorter than one 400 ms gating block"`, `"silent"`,
  `"required gain {g} hundredths exceeds -6000..=3600"` — are pinned distinct.
- **E46. The §5.6 / §5.8 pin tones are 997 Hz, not 1 kHz** (N1's rule): at 1 kHz the tone reads
  −29.99 LUFS and `gain == 1_600` would be 1 599; at 997 Hz the reading is exactly −3000.
- **E47. The hot-noise lane's pink noise is low-passed at 200 Hz and made up 3 dB.** The
  provisioned FFmpeg's `anoisesrc` pink is ~9 dB crest and would not reach the −3 dBTP ceiling
  after the move — the idle lane N4 Q2's self-checks exist to prevent. The lane's evidence is its
  asserted self-checks (`limiter_passes ≥ 1`, `peak_reduction > 0`, pre-encode peak within 0.5 dB
  of the ceiling), not a crest figure.
- **E48. The "no limiting needed" lane is the tone alone**, without §5.8's white noise 10 dB down:
  the provisioned `amix` halves both inputs whatever `normalize` says, which would make the lane's
  level a build fact. Its peak sits 11 dB under the ceiling; `peak_reduction == 0` and
  `limiter_passes == 1` are asserted.
- **E49. Fixture tones use `aevalsrc`, not `sine`** (the provisioned `sine` emits −18 dBFS, so
  `sine,volume=0.1` is a −38 dBFS programme); sources are `pcm_s16le` in `.mov` with the managed
  BT.709 encode arguments rather than AAC sources (an AAC source pre-smears the impulse lane and an
  untagged `testsrc2` is refused before any audio is mixed); the lanes export at the source raster,
  overriding `export_settings`' 1080p, since they gate audio only.
- **E50. The B8 verification fixture is 320 kbit/s** (192 kbit/s AAC adds 0.46 dB of true peak to a
  pure tone) and its decoded-tone term has its own budget,
  `VERIFY_TONE_TRUE_PEAK_BUDGET_HUNDREDTHS = 80` (observed 30), distinct from the hot lanes'
  `FIXTURE_AAC_OVERSHOOT_BUDGET_HUNDREDTHS`, which §5.8 F10 reserves for the pre-encode/decoded
  pair.
- **E51. `tests/au3_fixtures.rs` is not `cfg(feature = "test-util")`-gated**; an integration test
  cannot see a `src` feature-gated module, so it uses the `#[path = "../src/test_support.rs"]`
  include generated_media.rs uses. Consequence: the four lanes run in the default
  `cargo test -p kinewright-media` lane and add about 51 s to it.
- **E52. An out-of-range limiter ceiling is refused, not clamped.** `delivery_limiter_ceiling_tenth_db`
  returns a typed `MediaError::Backend` for a ceiling outside `−120..=0` tenth-dB (§5.6 step 5
  cites the range as a descriptor fact); it is −30 for every shipped target, asserted. The ceiling
  is validated before the master is gained, so a refused target leaves `mix` untouched (asserted
  by equality, not length), and the subtraction under it saturates. A master already on target
  takes a gain of exactly zero and still one limiter pass (pinned).
- **E53. `process_buffer_static`'s length guard fires only on unaligned input**, the only reachable
  form (`process_frame` works in place); `channels == 0` is a second typed refusal. Neither panics.
- **E54. B6 is three real exports** (off vs off for determinism, off vs skipped-`Some`, off vs
  acting-`Some`); comparing to `mix_audio`'s bytes is not observable through a lossy encode, and the
  third export is what stops the first two from being vacuous.
- **E55. The hot-noise LU margin is the exit gate's thinnest number** (deviation 37 hundredths
  against a 100 budget, ≥ 2× bar = 50), dominated by the limiter's loudness pull rather than the AAC
  round trip; the budget equals the streaming tolerance, so the bar is stricter than conformance. If
  the Windows lane reds on this term, the response is a per-OS note on the constant's doc comment
  in CC6 §6.3's manner, never a `cfg`.
- **E56. The corrective gain is not range-checked** against −6000..=3600 (§5.6 step 6 does not ask
  for it); there is no runaway — the pass count is a straight-line `if`, the correction fires only
  in the direction a limiter can produce, and the limiter re-clamps whatever the gain does.

## 1. Scope

### 1.1 The editor job

The editor from AU2 has a balanced, processed mix. They now need to see true loudness while playing
— momentary, short-term, integrated, range, true peak — pick a delivery profile whose loudness
target is known, have the export bring the programme to that target under a true-peak-safe ceiling,
and receive proof measured on the written file that it landed. The agent needs an audio QC report
it can act on (clipping, silence, channel balance, distance from target) and a normalization
planner that emits the real limiter.

### 1.2 The two parts

**Part A — Measurement.** `loudness.rs` rewritten around a per-chunk `LoudnessMeter` (D1) with the
five measurements (D2); `AudioLoudness`'s four new fields, `LoudnessSnapshot`, `Playback::loudness`
(D3); `MixObserver` on `mix_pass` (D4); `LoudnessTarget`, the profile match, and the headroom
constant (D5, F10, F12); `MediaError::MixLoudnessRangeTooShort` (Q3); `Analysis::audio_qc`,
`get_audio_qc`, `qa_document`'s mute/solo rule (D8); the Mixer `LOUDNESS` section and the master
strip line (D10a). **No export behaviour change** (`mix_pass`'s and `mix_audio`'s signatures move
in `export.rs`; A11 pins the bytes identical), **no `ExportSettings` field, no new operation.**

**Part B — Delivery.** `ExportSettings.loudness_normalization`, the step, `ExportAudioReport`,
`ExportReport`, `Export::export_document_reporting` (D6); `Analysis::verify_delivery_audio`,
`DeliveryAudioVerification` (D7); the planner on the true-peak limiter (D9);
`queue_export.normalize_loudness`, `ExportJobRecord.audio_report`/`audio_verification` (F9); the
export dialog `Loudness` row and `AUDIO` block (D10b).

**How the roadmap row is discharged.** The AU3 row's **exit gate** (ROADMAP-AND-WORKFLOWS.md:647)
has two clauses: (1) *"Encoded fixtures land within pinned LU/dBTP budgets on both CI operating
systems"* — closed by **Part B** (B12); (2) *"the QC report is integer-reported and evidence-only"*
— closed by **Part A** (A14, A15). The row's **deliverables** split the same way: Part A delivers
*"Momentary, short-term, integrated, LRA, and true-peak metering; per-profile loudness targets;
audio QC (`get_audio_qc`) with clipping, silence, channel-balance, and target checks"*; Part B
delivers *"normalization as an explicit export step"* and *"decoded verification of the written
file's loudness and true peak"*. Part A also closes AU1's standing debt *"QA (`qa.rs`) does not
consider mute/solo"* (AU1-MANUAL-MIX.md:89-91) and AU2's OPEN-4 (AU2-EQ-AND-DYNAMICS.md:276-281)
by Tech 3341 conformance rather than by the ITU table (§3.6).

### 1.3 Out of scope (named deferrals)

- Surround measurement: `BS1770_CHANNEL_WEIGHTS = [1.0, 1.0, 1.0, 1.41, 1.41]` is declared and
  never applied beyond two channels; export is stereo.
- Dialogue-gated or speech-anchored loudness; a spectrum display in the UI (AU4).
- Loudness targets as document state or per-project overrides; per-bus or per-stem normalization;
  AAC bitrate or codec changes; a processor checkpoint cache; re-encoding after verification.
- The ITU-R BS.1770-4 Annex 2 coefficient table (conformance is by Tech 3341 tolerance, D2); the
  limiter's own 4× `dsp::TruePeakEstimator` (dsp.rs:307-357) is unchanged.
- Streaming the decode side of `mix_pass` (R2).
- The planner's peak gate stays the **sample** peak its argument names (F17; §6.3).

### 1.4 Documentation

Part A: this contract; `CHANGELOG.md` (`### Added`; the partial-block removal and the short-range
refusal under `### Changed`); `MEDIA-POLICY.md` "Playback audio mixdown" (:106-141) gains one
paragraph on the observer and the audible-position live meter; `AU1-MANUAL-MIX.md:89-91` and AU2
§6.10's stem-memory and true-peak bullets gain AU3 pointers (AU1 §0 style, text not rewritten);
`DESIGN.md` Mixer section (§4.6); `M36-AGENT-RUNTIME-EFFICIENCY.md:111-112` gains a Part A row.

Part B: `README.md` lines 36 and 39; `CHANGELOG.md`; `ROADMAP-AND-WORKFLOWS.md` status paragraph
(:652-660) names the two parts and the clause split; `M34-CREATOR-DELIVERY-VERIFICATION.md:127`
amended (F21); `DESIGN.md` Dialogs (§6.8); `M36` Part B row.

---

# Part A — Measurement

---

## 2. Part A core model

### 2.1 `AudioLoudness` (D3)

`AudioLoudness` (media.rs:1043-1057) keeps its five fields and derives (`Copy`, `Eq`) and gains
four `Option<i32>` fields, each `#[serde(default, skip_serializing_if = "Option::is_none")]
#[schemars(default)]`: `momentary_max_lufs_hundredths` (loudest complete 400 ms window; `None`
under 400 ms or silent), `short_term_max_lufs_hundredths` (loudest complete 3 s window; `None`
under 3 s), `loudness_range_lu_hundredths` (Tech 3342; `None` under two gated windows),
`true_peak_dbtp_hundredths` (§3.6; `None` means digital silence). A pre-AU3 five-key JSON
deserialises with the four `None`; a value with all four `None` serialises to exactly the pre-AU3
string. Sample peak and true peak are reported for any non-empty programme, however short (F5).

### 2.2 `LoudnessSnapshot` and `Playback::loudness` (D3, F15)

```rust
/// Live loudness telemetry (§3.9). Integers only; not serialized, like `MixPeaks`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LoudnessSnapshot {
    pub momentary_lufs_hundredths: Option<i32>,
    pub short_term_lufs_hundredths: Option<i32>,
    pub integrated_lufs_hundredths: Option<i32>,
    pub loudness_range_lu_hundredths: Option<i32>,
    pub true_peak_dbtp_hundredths: Option<i32>,
    /// Whole seconds measured since the last reset, as of the audible position.
    pub programme_seconds: u32,
}
```

`Playback` (media.rs:1574-1600) gains `fn loudness(&self) -> LoudnessSnapshot {
LoudnessSnapshot::default() }` and `fn reset_loudness(&self) {}`, both defaulted, so no double
changes (facts-app §4).

### 2.3 `LoudnessTarget`, the profile match, and the headroom constant (D5, F10, F12, F16)

In `delivery.rs` beside `audio_bitrate` (delivery.rs:170-173):

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct LoudnessTarget {
    pub integrated_lufs_hundredths: i32,
    pub tolerance_lu_hundredths: i32,
    pub true_peak_ceiling_dbtp_hundredths: i32,
    #[serde(default, skip_serializing_if = "Option::is_none")] #[schemars(default)]
    pub loudness_range_max_lu_hundredths: Option<i32>,
}
/// EBU R128 programme loudness: −23 LUFS ±1 LU, −1 dBTP; R128 treats LRA as a descriptor, so no
/// maximum. ATSC A/85 and ARIB TR-B32 (−24 LKFS) are the regional alternatives; a project that
/// delivers to them picks the value by hand until targets become document state.
pub const EBU_R128_PROGRAMME_TARGET: LoudnessTarget = /* −2_300, 100, −100, None */;
/// The streaming and social norm: platforms normalise to −14 LUFS and ask for −1 dBTP so a lossy
/// re-encode does not clip. No platform publishes a tolerance; ±1 LU is a house number equal to
/// the measurement's own resolution on short programmes.
pub const STREAMING_PLATFORM_TARGET: LoudnessTarget = /* −1_400, 100, −100, None */;
/// The true-peak headroom held under a target's ceiling before a lossy encode, shared by the
/// export step (§5.6) and the planner (§6.3). Re-baselined only from the §5.8 hot lanes' printed
/// pre-encode and decoded true peaks, in CC6 §6.3's manner.
pub const LOSSY_CODEC_TRUE_PEAK_HEADROOM_HUNDREDTHS: i32 = 200;
impl DeliveryProfile {
    #[must_use] pub const fn loudness_target(self) -> LoudnessTarget { match self {
        Self::SourceMaster => EBU_R128_PROGRAMME_TARGET,
        Self::Youtube1080p | Self::VerticalShort | Self::SquareSocial => STREAMING_PLATFORM_TARGET } }
}
```

The agent's `LOSSY_CODEC_PEAK_HEADROOM_HUNDREDTHS` (server.rs:127-131) is deleted in favour of the
core constant. Targets are profile facts in the same `match self` as `audio_bitrate`; the
orthogonal job axis is `ExportSettings.loudness_normalization` (Part B). `delivery_conformance`
(delivery.rs:320-381) and `export_ready` never read a target.

### 2.4 Audio QC types (D8, F13, F14, Q5, Q3)

New `crates/kinewright-core/src/audio_qc.rs`, re-exported from `lib.rs`:

```rust
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AudioQcRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")] #[schemars(default)]
    pub range: Option<std::ops::Range<TimeCode>>,
    #[serde(default, skip_serializing_if = "Option::is_none")] #[schemars(default)]
    pub profile: Option<DeliveryProfile>,
}
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AudioChannelClipping {
    /// Samples of this channel at |x| >= 1.0 on the pre-clamp master.
    pub over_full_scale_samples: u64,
    /// Runs of >= AUDIO_QC_CLIPPED_RUN_SAMPLES consecutive such samples on this channel.
    pub clipped_runs: u32,
    /// floor(over_full_scale_samples * 10_000 / this channel's sample count); 0 when empty.
    pub basis_points: u32,
}
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AudioClipping { pub left: AudioChannelClipping, pub right: AudioChannelClipping }
/// ColorQcException's shape (color_qc.rs:633-653) under its own name, without clip/effect.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AudioQcException { pub code: String, pub severity: QaSeverity, pub message: String,
    /* field, observed, allowed: Option<String>, each serde default + skip_none + schemars default */ }
/// Fixed strings; a manual `impl Default` (no derive) so the eight values are one literal each.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AudioQcProvenance { pub engine: String, pub measurement_rate: u32, pub k_weighting: String,
    pub true_peak: String, pub true_peak_bias: String, pub gate: String, pub loudness_range: String,
    pub silence: String, pub exception_order: String }
pub const AUDIO_QC_ENGINE: &str = "kinewright_audio_qc_v1";
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AudioQcReport {
    pub range: std::ops::Range<TimeCode>,
    /// The post-clamp master: the signal export encodes.
    pub master: AudioLoudness,
    /// 10·log10(E_L/E_R) over the integrated gate's block set, clamped to ±6000 (§3.10).
    pub channel_balance_lu_hundredths: Option<i32>,
    pub clipping: AudioClipping,
    pub leading_silence_frames: TimeCode,
    pub trailing_silence_frames: TimeCode,
    pub target: Option<LoudnessTarget>,
    pub exceptions: Vec<AudioQcException>,
    /// No Error-severity exception.
    pub technical_pass: bool,
    /// Always true.
    pub evidence_only: bool,
    pub provenance: AudioQcProvenance,
}
pub const AUDIO_QC_SILENCE_DBFS_HUNDREDTHS: i32 = -7_000;
pub const AUDIO_QC_SILENCE_WINDOW_MILLISECONDS: u32 = 10;
pub const AUDIO_QC_SILENCE_INFO_MILLISECONDS: u32 = 1_000;
pub const AUDIO_QC_CLIPPED_RUN_SAMPLES: u32 = 3;
pub const AUDIO_QC_CHANNEL_IMBALANCE_LU_HUNDREDTHS: i32 = 300;
pub const AUDIO_QC_CHANNEL_BALANCE_CLAMP_LU_HUNDREDTHS: i32 = 6_000;
```

`AudioQcProvenance::default()` (the manual impl) records `measurement_rate: 48_000`, `k_weighting:
"bs1770_4_bilinear_from_prototypes"`, `true_peak: "8x_polyphase_256_tap_blackman_harris"`,
`true_peak_bias: "exact_on_grid_at_most_-0.042_db_between_grid_points"`, `gate:
"bs1770_4_two_stage_complete_blocks"`, `loudness_range: "ebu_tech_3342_nearest_rank"`, `silence:
"10ms_rms_-70_dbfs_post_clamp"`, `exception_order: "severity_desc_code_asc_field_asc"`.

Thresholds are consts, not parameters (CC6's `_BASIS_POINTS` rule, color_qc.rs:275-288). One core
function `audio_qc_exceptions(...)` builds the list from media's measured numbers, and one shared
`loudness_target_exceptions(measured, target, prefix)` is reused by §5.3 with prefix `delivery`.
Because a range shorter than one gating block is refused before measurement (§2.5, Q3), after a
refusal-free measurement `integrated == None` means the range was gated out entirely — silent —
and nothing else:

| code | severity | raised when | `field` / `allowed` |
| --- | --- | --- | --- |
| `audio_silent` | **Warning** (Q5) | `master.integrated == None` | `integrated_lufs_hundredths` / `> -7000` |
| `audio_clipping` | Error | `left.clipped_runs + right.clipped_runs > 0` | `clipping.{left,right}.clipped_runs` / `0` |
| `audio_true_peak_over_ceiling` | Error | target and `true_peak > ceiling` | `true_peak_dbtp_hundredths` / `<= {c}` |
| `audio_loudness_out_of_tolerance` | Warning | target and `\|I − t\| > tol` | `integrated_lufs_hundredths` / `{t−tol}..={t+tol}` |
| `audio_loudness_range_over_maximum` | Warning | target has a max and LRA exceeds it | `loudness_range_lu_hundredths` / `<= {max}` |
| `audio_channel_imbalance` | Warning | `\|Δ\| > 300`, or Δ `None` with `J ≠ ∅` and one side zero | `channel_balance_lu_hundredths` / `-300..=300` |
| `audio_leading_silence` | Info | leading silence `> 1 000 ms` | `leading_silence_frames` / `<= 1 s` |
| `audio_trailing_silence` | Info | trailing silence `> 1 000 ms` | `trailing_silence_frames` / `<= 1 s` |

Order `(severity desc, code asc, field asc)` — `color_qc.rs:1103`'s comparator without the clip and
effect terms. `technical_pass` is the absence of an `Error`. `audio_silent` suppresses the target,
balance, and silence entries (`Δ = None` for `J = ∅` raises nothing, F3).

### 2.5 `Analysis::audio_qc` and `MediaError::MixLoudnessRangeTooShort` (D8, Q3)

`Analysis` (media.rs:1602-1933) gains, after `mix_spectrum` (:1857-1865), `fn audio_qc(&self,
document: &Document, request: &AudioQcRequest) -> Result<AudioQcReport, MediaError>` defaulting to
`NotImplemented`. `MediaError` (media.rs:1499-1554) gains, beside `MixSpectrumRangeTooShort`
(:1552), `#[error("mix loudness needs at least {required} sample frames; got {sample_frames}")]
MixLoudnessRangeTooShort { sample_frames: u64, required: u64 }`, `recovery_code() == None` like its
sibling. `mix_levels` and `audio_qc` return it for a clamped range under `LOUDNESS_GATING_BLOCK_FRAMES
= 19_200` (one 400 ms block at 48 kHz) **before decoding**, exactly as `measure_mix_spectrum` refuses
(export.rs:1102). `timeline_loudness`, `asset_loudness`, and `verify_delivery_audio` measure whole
programmes and are not refused: a short programme reads `integrated == None` with its peaks `Some`
(the eval's 5-frame tail document reads only the sample peak, eval.rs:2545-2549).

### 2.6 `qa_document`'s mute/solo rule (F22)

`has_audible_media` (qa.rs:211-215) additionally requires `document.track_audible(track.id)`
(model.rs:1128-1131). The `no_audible_media` issue (qa.rs:338-347) keeps its code and Info severity
and gains a second message: when at least one clip would have qualified but every such track is
inaudible, *"Every audio-bearing track is muted or silenced by another track's solo."*; otherwise the
existing sentence. Info never reaches `export_ready` (qa.rs:52-55).

### 2.7 Part A contract tests

New `crates/kinewright-core/tests/au3_core.rs` in `au2_core.rs`'s style (claim-sentence names,
`/// AU3 §7 item` docs); `contracts.rs` gains wire-shape pins. Items A1–A4.

## 3. Part A media

### 3.1 `LoudnessMeter` (D1, F6, F8)

`loudness.rs` is rewritten around one type; `measure_loudness` (loudness.rs:86-186) becomes
`LoudnessMeter::new(rate, ch)?; push(buf)?; finish()` so lib.rs:92, engine.rs:773/788, and the
audio.rs:4192 `assert_eq!` pin keep compiling and passing.

```rust
pub struct LoudnessMeter { /* K filters, open sub-block sums, per-channel sub-block energies keyed by
                              end sample, oversampler, peaks */ }
impl LoudnessMeter {
    /// `sample_rate >= 1_000`, `channels` 1 or 2; else `MediaError::Backend`.
    pub fn new(sample_rate: u32, channels: u16) -> Result<Self, MediaError>;
    /// Whole interleaved frames. A slice not aligned to the channel count is refused with
    /// `MediaError::Backend("interleaved PCM is not aligned to its channel count")` (loudness.rs:102-106's
    /// message) and nothing is consumed. Returns the number of 100 ms sub-blocks completed by this push.
    pub fn push(&mut self, interleaved: &[f32]) -> Result<usize, MediaError>;
    /// The live shape over every completed sub-block. O(B log B) in the sub-block count B (the LRA sort).
    pub fn snapshot(&self) -> LoudnessSnapshot;
    /// End sample (frames since reset) of the last completed sub-block; `None` before the first.
    pub fn last_block_end(&self) -> Option<u64>;
    /// Drop completed sub-blocks whose end sample is past `frames`, discard the open sub-block, and
    /// restart the filter and oversampler state (§3.9 pause). Peaks are kept.
    pub fn truncate_to(&mut self, frames: u64);
    /// Flush the oversampler (§3.6) and convert. Consumes the meter.
    pub fn finish(self) -> Result<AudioLoudness, MediaError>;
    pub fn measure(samples: &[f32], sample_rate: u32, channels: u16) -> Result<AudioLoudness, MediaError>;
    pub fn reset(&mut self);
    pub fn sample_frames(&self) -> u64;
    pub fn channel_balance_lu_hundredths(&self) -> Option<i32>;      // §3.10
}
```

State: two DF-I `f64` biquads per channel (loudness.rs:7-39, kept); one running `f64` square sum
per channel for the open sub-block; a `Vec<f64>` of completed sub-block energies per channel (one
`f64` per 100 ms per channel: 6 000 × 2 × 8 B = 96 KB at ten minutes); the true-peak history; the
running sample and true peaks. Every accumulation is sequential per sample in `f64` with filter and
FIR state carried across `push`, so the result is **independent of frame-aligned chunking**: one
push, 1 024-frame pushes, and 7-frame pushes of the same programme produce bit-identical `finish()`
output (A5, F6); a misaligned push is refused rather than silently trimmed (F8).

### 3.2 K-weighting derivation (N1, F1, R3)

Both stages are the bilinear transform of the BS.1770-4 analogue prototypes, `f64`, computed at
construction for the meter's rate — the De Man parametrisation, **not** the RBJ cookbook shelf
(which misses Table 1 by up to 0.056, on `b1`) nor the cookbook's normalised high-pass numerator:

```text
Stage 1 — pre-filter (high shelf):  f0 = 1681.974450955533 Hz, Q = 0.7071752369554196,
                                     G  = +3.999843853973347 dB
  K  = tan(π·f0/fs)      Vh = 10^(G/20)      Vb = Vh^0.4996667741545416      a0 = 1 + K/Q + K²
  b  = [ (Vh + Vb·K/Q + K²)/a0,  2·(K² − Vh)/a0,  (Vh − Vb·K/Q + K²)/a0 ]
  a  = [ 1,  2·(K² − 1)/a0,  (1 − K/Q + K²)/a0 ]
Stage 2 — RLB (high-pass):          f0 = 38.13547087602444 Hz, Q = 0.5003270373238773
  K  = tan(π·f0/fs)      a0 = 1 + K/Q + K²
  b  = [ 1, −2, 1 ]                       unnormalised, as the ITU table (|H| at Nyquist = 1.00499)
  a  = [ 1,  2·(K² − 1)/a0,  (1 − K/Q + K²)/a0 ]
```

**Pins (A6).** At 48 kHz the derivation reproduces BS.1770-4 Tables 1–2 — today's literals at
loudness.rs:43-54 — within **1e-6** on every coefficient (achieved 8.9e-16 / 2.2e-16). Secondary
pins within 1e-6: 44.1 kHz shelf `b = [1.53084123, −2.65098000, 1.16907908]`, `a = [1, −1.66365511,
0.71259543]`, RLB `a = [1, −1.98916967, 0.98919904]`; 96 kHz shelf `b = [1.55971423, −2.92674158,
1.37826120]`, `a = [1, −1.84460947, 0.85584332]`, RLB `a = [1, −1.99501754, 0.99502376]`.
**Calibration, at 997 Hz — this contract's calibration tone, chosen because it is not commensurate
with any sub-block and so exercises the filters off-grid** (the Tech 3341 pins of §3.3–3.5 use
1 kHz, as the standard specifies): `|H_K(997 Hz)|` is +0.69101 / +0.69378 / +0.67369 dB at 48 /
44.1 / 96 kHz, which `LOUDNESS_OFFSET = −0.691` (loudness.rs:5) cancels, so a stereo 0 dBFS 997 Hz
sine reads **0 / 0 / −2** hundredths exactly (R3), a one-channel 0 dBFS 997 Hz sine reads −301 at
48 kHz, and a stereo 997 Hz sine at **peak amplitude `10^(−23/20) = 0.0708` on both channels** reads
−2300 ±10 (F2: the two channels' +3.01 dB cancels the sine's −3.01 dB crest factor; a one-channel
tone needs +3.01 dB).

### 3.3 Blocks, complete-block gating, and the integrated value (D1, D2, F5, F16)

`S = sample_rate / 10` frames is the **sub-block** (4 800 at 48 kHz, 4 410 at 44.1 kHz). For
channel `i` and sub-block `s`, `z_{i,s} = (1/S)·Σ y²` over the S K-weighted samples. Only
**complete** sub-blocks exist. A **block** — the 400 ms gating block at 75 % overlap, which is also
the momentary window — is four consecutive sub-blocks ending at `j ≥ 3`:

```text
m_j = Σ_i G_i · (z_{i,j−3} + z_{i,j−2} + z_{i,j−1} + z_{i,j}) / 4          G_i = 1 for i < 2
l_j = −0.691 + 10·log10(m_j)
J_g = { j ≥ 3 : l_j > −70 }                                 → None when empty
Γ_r = −0.691 + 10·log10( mean_{j∈J_g} m_j ) − 10
J   = { j ∈ J_g : l_j > Γ_r }                               → None when empty     (strict >, both)
L_K = −0.691 + 10·log10( mean_{j∈J} m_j )                    → hundredths(L_K)
```

This is loudness.rs:161-177's algebra with the **partial block removed** (today :136-159 includes a
final short block normalised by its own length and measures a sub-400 ms signal as one short block,
:131-132). **Behaviour change, recorded:** a 399 ms programme (19 152 frames, three sub-blocks)
reads `integrated == None` and a 400 ms one (19 200 frames, `j = 3`) `Some` (F5 pin); the last
`< 100 ms` of any programme falls outside the sub-block grid, and the first 300 ms are covered by
fewer blocks than the interior, as in every 75 %-overlap BS.1770 meter (F16); the AU1/AU2 level
fixtures move by under a hundredth; no existing test pins an absolute LUFS value (facts-media §1);
the eval's activity window is 25 frames ≥ 400 ms (`benchmarks/auto-edit/v5/music-ground-truth-v9.
json:96`). With Q3's refusal on `mix_levels`, the only pre-AU3 caller of a sub-block range is the
A16 impulse test (audio.rs:5681, `TimeCode::ZERO..TimeCode(1)` at 10 fps = 4 800 frames): its range
becomes `0..4` — exactly one gating block, still containing the frame-0 impulse — and its three
`sample_frames` assertions read 19 200; its peak assertions are unchanged. `get_audio_levels`'
description (server.rs:9992) keeps "which is what a muted or solo-suppressed track reports" and
adds "a range shorter than one 400 ms gating block is refused". The three consts and `hundredths`
(loudness.rs:3-5, :62-71) are unchanged.

**Tech 3341 integrated pins (A7)**, all **1 kHz** stereo as the standard specifies (48 samples per
cycle, so every sub-block energy is exact), levels as peak dBFS per channel, ±10 hundredths: case 1
−23 dBFS 20 s → −2300; case 2 −33 → −3300; case 3 −36 / −23 / −36 at 10 / 60 / 10 s → −2300
(relative gate: ungated mean −24.18, gate −34.18 removes the −36 blocks); case 4 adds −72 dBFS 10 s
head and tail → −2300 (absolute gate); case 5 −26 / −20 / −26 at 20 / 20.1 / 20 s → −2300 (nothing
gated; the 20.1 s lands the mean).

### 3.4 Momentary and short-term (D2, F9)

Momentary at `j ≥ 3` is `l_j`. Short-term at `j ≥ 29` is `l^s_j = −0.691 + 10·log10(s_j)`,
`s_j = Σ_i G_i · (z_{i,j−29} + … + z_{i,j}) / 30`. Neither is gated. The maxima are over complete
windows with non-zero energy; `None` otherwise. The live snapshot's momentary and short-term are the
latest complete values at the audible position (§3.9). **Pins (A8)**, 1 kHz stereo: Tech 3341
case 12 (alternating −20 dBFS 0.18 s / −30 dBFS 0.22 s): every momentary reading after the first
complete window is −2300 ±10 (each 400 ms window covers exactly one period); case 9 (−20 dBFS
1.34 s / −30 dBFS 1.66 s): every short-term reading after the 30th sub-block is −2300 ±10. Window
and hop, this contract's own: 1 s of silence then a −20 LUFS tone: momentary `j = 12` reads `−2000
+ 1000·log10(3/4) = −2125` ±1 and `j = 13` −2000 ±1; short-term `j = 38` reads `−2000 +
1000·log10(29/30) = −2015` ±1 and `j = 39` −2000 ±1; the series have `sub_blocks − 3` and
`sub_blocks − 29` entries (test-only accessor).

### 3.5 Loudness range — EBU Tech 3342 (D2, F4, F6)

On the short-term series (3 s window, 100 ms hop; the standard permits any overlap ≥ 66 %, and the
pinned expectations depend on the hop only through the transition-window population):

```text
A = { j : −0.691 + 10·log10(s_j) > −70 }                      absolute gate on the short-term series
Γ = −0.691 + 10·log10( mean_{j∈A} s_j ) − 20                  −20 LU below the POWER MEAN of A's windows
B = { j ∈ A : −0.691 + 10·log10(s_j) > Γ }                    n = |B|; n < 2 → None
sort l^s_j for j ∈ B ascending into l[0..n)
idx(p) = min(n − 1, ceil(p·n) − 1)                             CC6 §10.4's nearest rank
LRA = l[idx(0.95)] − l[idx(0.10)]  in LU                       → hundredths
```

**Tech 3342 pins (A9)**, 1 kHz stereo, 20 s per level, expected ±10 hundredths (the standard allows
±1 LU; plateaus make it exact). Each 20 s plateau has 171 short-term windows wholly inside it and
each boundary 29 transition windows holding `k/30` of the later level, `k = 1..29`: case 1 −20 / −30
→ **1 000** (371 windows; `Γ = −42.60`; `idx(0.10) = 37` in the −30 plateau, `idx(0.95) = 352` in
the −20 plateau); case 2 −20 / −15 → **500**; case 3 −40 / −20 → **2 000**; case 4 −50 / −35 / −20
/ −35 / −50 → **1 500** (power mean over the 971 windows −26.59, `Γ = −46.59`, removes the −50
plateaus and admits 56 of the 58 −50/−35 transitions; `n = 627`; `idx(0.10) = 62` is the seventh
−35 window, `idx(0.95) = 595` is −20; ungated 3 000). This contract adds a relative-gate case with
an analytic non-plateau answer: −60 / −15 (20 s each): the power mean over windows is exactly
`E_hi/2` (coverage `185.5 × 30` of `371` windows; `Γ = −38.01`), the −60 plateau is removed,
`n = 200`, `idx(0.10) = 19` is transition `k = 20` at `−15 + 10·log10(20/30) = −16.76`, `idx(0.95)
= 189` is −15 → **176**; ungated **4 500**, asserted in-test.

### 3.6 True peak (D2, F3/Q1, R1, R5, F7)

An **8× polyphase windowed-sinc oversampler, 32 taps per phase, 256 taps**, `f64`, run per channel
over the buffer under measurement, with the phase grid aligned to the input samples:

```text
N = 256, OVERSAMPLE = 8, PHASE_TAPS = 32, centre tap c = 128
for n in 0..256:
    x_n  = (n − 128) / 8
    w_n  = 0.35875 − 0.48829·cos(2πn/256) + 0.14128·cos(4πn/256) − 0.01168·cos(6πn/256)
    h[n] = sinc(x_n)·w_n                        sinc(0) = 1, sinc(x) = sin(πx)/(πx)
each phase p in 0..8 is normalised so that Σ_{k=0}^{31} h[8k + p] = 1     (raw sums within 1.2e-6 of 1)
y_p[i] = Σ_{k=0}^{31} h[8k + p]·x[i − k]       evaluates the bandlimited signal at t = i − 16 + p/8
TRUE_PEAK_METER_GROUP_DELAY_FRAMES = 16          exact on this grid ((N−1)/2/8 = 15.94 on a half-sample grid)
```

Phase 0 is the identity delayed by 16 frames: `w(128) = 0.35875 + 0.48829 + 0.14128 + 0.01168 =
1.0` exactly, so `h[128] = 1.0`, and every other `h[8k] = sinc(k − 16)·w` is `sin(πk)/(πk)` in
`f64`, about 4e-17 (F7). The sample peak is therefore structurally in the estimate (R5; the AU2
detector's `(n − 23.5)/4` grid never touches a sample, AU2 E12). The true peak is the maximum of
`|y_p[i]|` over phases, emitted frames, and channels; `true_peak_dbtp_hundredths =
hundredths(20·log10(peak))`, `None` when zero. Taps in a `OnceLock`. Cost 256 MACs per input frame
per channel: 24.6 M MAC/s live at 48 kHz stereo, about 15 s of `f64` for a ten-minute programme
offline (F3). **Stated bias:** a tone whose crest lies between two grid points is read under by at
most `20·log10(cos(π/32)) = −0.042 dB` (measured −0.037 at fs/4 over an 87° phase sweep, −0.033 at
20 kHz); never over; recorded in `provenance.true_peak_bias` and §6.9.

**Edges.** Zero-initialised history; `finish()` feeds **32 zero frames** so every real frame has
been seen by all eight phases (an impulse at the last sample reads 0 dBTP with the flush and −105
without). The live true peak (§3.9) is the running maximum over everything interpolated so far.

**Conformance pins (A10), Tech 3341 by tolerance.** Every tone is 50 ms at 48 kHz with a 10 ms
raised-cosine fade in and out (R1). Cases 15–18 at amplitude 0.5 — fs/4 at 0°, fs/4 at 45°, fs/6
at 60°, fs/8 at 67.5° — read **−602 within +20/−40** (design check: −6.0206 on all four); 997 Hz
and 20 kHz at 0.5 read −602; a full-scale impulse at the first, middle, and last sample reads
exactly 0; an fs/4 tone swept over sample phases 0°–87° in 3° steps never reads under −5;
broadband noise reads within **2 hundredths** of a 16×/512-tap reference written in the test (the
audio.rs:4915-4954 pattern; measured −2.178 vs −2.171) with the bias printed; the eight raw phase
sums are within 2e-6 of 1; `h[128] == 1.0` and `|h[8k]| < 1e-15` for `k ≠ 16` (N4: asserted to
1e-15, not to equality). `dsp::TruePeakEstimator` and AU2 A11's ±0.3 pin are untouched;
dsp.rs:307-312 gains "the measurement path in `loudness.rs` is an 8× meter conformant by Tech 3341
tolerance (AU3 §3.6)".

### 3.7 Numeric discipline

`f64` inside the meter for every filter, sum, mean, logarithm, and tap; `f32` only at `push`. Every
output is `i32` hundredths through `hundredths()` (round half away from zero, range-checked), `u64`
for counts, `Option` for unmeasured (CC6:1070). No AU3 API returns an `f32` or `f64` to an agent,
a queue record, or the UI; the app derives bar fills from hundredths itself (§4.4).

### 3.8 `MixObserver` and the streamed feed (D4, F7)

```rust
pub(crate) trait MixObserver {
    /// True when the pass must run `mix_chunk_with_stems` (audio.rs:1886-1893); `mix_audio` pays nothing.
    fn wants_stems(&self) -> bool { false }
    fn track(&mut self, _track: TrackId, _chunk: &[f32]) -> Result<(), MediaError> { Ok(()) }
    fn bus(&mut self, _bus: AudioBusId, _chunk: &[f32]) -> Result<(), MediaError> { Ok(()) }
    /// The summed master BEFORE the single clamp (F7); a loudness consumer feeds `clamp(chunk)` to its meter.
    fn master(&mut self, _chunk: &[f32]) -> Result<(), MediaError> { Ok(()) }
}
pub(crate) struct NoObserver;
```

`mix_pass` (export.rs:808-999) gains `observer: &mut dyn MixObserver` and `collect: MixCollect {
stems: bool, master: bool }` in place of `collect_stems`. `mix_audio` (:789-794) passes `{ stems:
false, master: true }` and `NoObserver`; `mix_audio_stems` (:799-805) keeps `{ true, true }` for
`measure_mix_spectrum` (:1092-1141), **the remaining whole-stem path**; `measure_mix_levels` and
`audio_qc` pass `{ false, false }` with their observers. The post-loop clamp (:946) stays where it
is; the observer sees each 1 024-frame chunk (:920) inside the loop (:916-945, `mix_chunk_with_stems`
at :933 and `mix_chunk` at :942) before it.

**Head drop and tail truncation on a streamed feed.** AU2 §3.7's trims (export.rs:951-988) become
a per-family window, applied to the feed and never to the observer:

```rust
struct FamilyWindow { skip: usize, remaining: usize }        // sample frames
impl FamilyWindow {
    fn take<'a>(&mut self, chunk: &'a [f32], ch: usize) -> &'a [f32] {
        let n = chunk.len() / ch;
        let drop = self.skip.min(n); self.skip -= drop;
        let take = (n - drop).min(self.remaining); self.remaining -= take;
        &chunk[drop * ch..(drop + take) * ch]
    }
}
```

| family | `skip` | `remaining` | today's stem trim |
| --- | --- | --- | --- |
| track | `0 + K` | `T` | no head (:962-965), `keep_from` (:956-961), truncate (:983-988) |
| bus | `bus_stage_frames + K` | `T` | `bus_head` (:952-955), `keep_from`, truncate |
| master | `latency + K` | `T` | `latency` (:951), `keep_from`, truncate |

with `K = frame_to_samples(range.start)` and `T = frame_to_samples(range.end) − K`. A chunk
straddling a boundary is split; the observer receives exactly the frames the stem would have held,
in order. `measure_mix_levels` (:1007-1058) becomes a `LevelsObserver` (`wants_stems: true`) of one
`(FamilyWindow, LoudnessMeter)` per track, per bus, and for the master, replacing the three
`measure_loudness` calls (:1033, :1047, :1050), after the Q3 length refusal. **Pins (A11).** Through
the observer, `measure_mix_levels` is `assert_eq!` — the whole `AudioLoudness` — to
`LoudnessMeter::measure` over the corresponding `mix_audio_stems` family on every AU1/AU2 level
fixture and on `impulse_document` (audio.rs:5587);
`measure_mix_levels_trims_every_stem_family_to_the_requested_range` (audio.rs:5681) passes with its
range widened to one block (§3.3); `mix_audio` on `parity_document_with_master_chain` is
bit-identical to today's; a chunk split across `range.start` feeds exactly `T` frames.

### 3.9 The live meter, published by audible position (D3, F15/Q4, N4 Q1, F5, F10)

**Ownership.** The `Worker` (engine.rs:1279-1307) owns a `LoudnessMeter::new(device_rate,
min(output_channels, 2))` — a one-channel device is measured as mono, channels beyond the second are
ignored — that outlives the `AudioMixer` (rebuilt on every play, engine.rs:1531) and the
`AudioRuntime` (engine.rs:1303, `self.audio`). It is threaded, not shared: `AudioRuntime::new(..,
meter: &mut LoudnessMeter)` for the initial fill (audio.rs:2523) and `AudioRuntime::fill(&mut self,
meter: &mut LoudnessMeter)` (audio.rs:2546-2554; called from the worker tick and `fill_audio`, engine.rs:1587, :1659) pass it to `fill_ring(.., meter: &mut
LoudnessMeter)` (audio.rs:2427-2456), which pushes the first two channels of each post-clamp chunk
it takes from `mixer.next_chunk()`; `fill_ring`'s three test callers (audio.rs:3844, :3865, :3884)
pass a scratch meter. **The meter is fed at ring-fill time, up to `LIVE_FILL_MILLISECONDS = 1_000`
(audio.rs:35, :2508-2513) ahead of the loudspeaker** (F2), so nothing it holds is published as
current until the sound has been heard:

- **Ring.** Whenever `push` reports a completed sub-block the worker records `(meter.last_block_end(),
  meter.snapshot())` into a 16-entry ring (the lead is ≤ 1 s = ≤ 10 sub-blocks; 16 leaves headroom
  for a slow tick). Keys are device sample frames since the meter's reset.
- **Publish.** On its 5 ms tick (`WORKER_TICK`, engine.rs:44; the `recv_timeout` loop at :1350) the
  worker converts the device clock position (`SharedClock::position`, engine.rs:77-91, driven by
  consumed samples) to frames since reset — `frame_to_samples(position − reset_frame, device_rate,
  fps)` — and stores the ring entry with the greatest key `≤` that count into the six atomics
  below. The `M`/`S` bars therefore track the loudspeaker, and integrated / LRA / `programme_seconds`
  describe the **heard** prefix.
- **Pause.** `Worker::pause` (engine.rs:1554-1571) reads `paused_at = clock.position()` (:1560 — the
  same value stored to `fallback_frame` and returned by `Playback::position()`, :77-80, :671-673),
  then calls `meter.truncate_to(frames(paused_at))`: completed sub-blocks past the audible position
  are dropped, the open sub-block is discarded, and filter and oversampler state restart, so the
  pre-rendered second is neither counted twice nor lost; the published entry is the one at
  `paused_at`, with `momentary = short_term = None` (the windows describe audio no longer
  playing) and the other four fields readable.
- **Continue and reset (F15, F10).** `start_playback` (engine.rs:1517-1531) clamps `from` to
  `duration − 1` (:1529); the meter **continues** when the clamped `from == paused_at` (the app's
  resume passes `Playback::position()`, transport.rs:23-24 via app.rs:1315-1316, so this holds
  today) and its filters restart at the resume point (a 4 ms transient, stated); it **resets** —
  meter, ring, atomics to `default()` — on `seek` (engine.rs:664-668 → the worker's play path
  :1520), on `play(from)` with `from ≠ paused_at`, and on `Playback::reset_loudness()`
  (`Control::ResetLoudness`). A device-rate change rebuilds the meter.

```rust
/// AU3 §3.9: written once per publish. i32::MIN is "none"; `energy_loudness(0)` maps to it.
/// A torn read pairs fields from different publishes; each field is individually current or newer and
/// none is stale, accepted (F15) because the six values are displayed, never combined.
pub(crate) struct LiveLoudness { momentary: AtomicI32, short_term: AtomicI32, integrated: AtomicI32,
    loudness_range: AtomicI32, true_peak: AtomicI32, programme_seconds: AtomicU32 }
```

Release stores on the worker thread (`fill_audio`, engine.rs:1654, and the tick — never the cpal
callback), Acquire loads in `Playback::loudness`, sentinels mapped to `None`. `MeterState`'s two
atomics (audio.rs:155-194) overwrite per chunk at fill time and lead the sound; these six hold per
block at the audible position. `programme_seconds = floor(published_key / rate)`. **Pins (A12)**,
driving `fill_ring` and the publish step with a simulated clock: a −20 LUFS tone converges to −2000
±20 within 3 s of *heard* programme and true peak reads the amplitude ±1; with the clock held 1 s
behind the fill, the published integrated lags the meter's own by ≤ 10 sub-blocks and never leads;
pause after N seconds then continue at `paused_at`: `programme_seconds` and integrated exclude the
pre-rendered second exactly once (equal to an uninterrupted run within ±1 hundredth); `momentary ==
None` while paused; play elsewhere, seek, and `reset_loudness` reset to `default()`; under 3 s
`short_term == None` and under 400 ms `integrated == None` in the live shape.

### 3.10 `Analysis::audio_qc` in media (D8, F13, F14, F8, Q3)

`FfmpegMediaEngine::audio_qc` calls `export::measure_audio_qc(document, request)`:
`clamped_measurement_range` (export.rs:1065-1079) → the Q3 refusal when the clamped range holds
fewer than 19 200 frames → `measurement_settings` (:1144-1155) → one `mix_pass` with `QcObserver`
(`wants_stems: false`), whose `master(pre_clamp)` callback:

- counts clipping **per channel** on the pre-clamp chunk: a sample at `|x| ≥ 1.0` increments that
  channel's `over_full_scale_samples`; `≥ 3` consecutive such samples on one channel close one run
  (run state carried across callbacks); `basis_points = floor(over · 10 000 / channel_samples)`;
- clamps a copy (`limit_audio_mix`, audio.rs:149-153) and pushes it to the `LoudnessMeter`
  (`report.master`, balance) and to the silence windows.

**Channel balance (F14).** One block set: the integrated gate's `J` (§3.3), gated on the summed
loudness. `E_L = Σ_{j∈J} Σ_{s∈j} z_{0,s}`, `E_R` likewise; `Δ = hundredths(10·log10(E_L/E_R))`
clamped to ±6 000; `None` for mono, for `J = ∅` (raises nothing beyond `audio_silent`), or when
`J ≠ ∅` and either energy is zero — in which case `audio_channel_imbalance` is raised with
`observed: "none (one channel silent)"`.

**Silence, sample domain (F13, F8).** `detect_silences`' windowing (derived.rs:622-639):
non-overlapping windows of `ceil(48 000 · 10 / 1 000) = 480` frames over the post-clamp master,
RMS over the window's interleaved samples (both channels), threshold `10^(−7000/2000)`. **The
window accumulator (square sum and count) persists across `master()` callbacks**: `1 024 = 2 × 480
+ 64`, so every chunk boundary splits a window, and a window is closed only when its 480th frame
arrives; the one partial window measured over its own length is the one at `range.end`, never at
a chunk boundary. `leading_silence_frames = TimeCode(floor(n_lead · 480 · fps_num / (48 000 ·
fps_den)))` over the leading run of windows at or under the threshold; trailing symmetric. Infos
when a run exceeds 1 000 ms (100 windows).

`target = request.profile.map(DeliveryProfile::loudness_target)`. **Pins (A13, A14).** A full-scale
1 kHz stereo tone with a `+6 dB` `audio_gain` on the master: both channels' `clipped_runs ≥ 1`,
`audio_clipping` Error, `technical_pass == false`, `master.sample_peak == 0` (the clamp); an L-only
tone: `Δ == None`, `audio_channel_imbalance` Warning; L at −20 and R at −26 LUFS: `Δ == 600 ±5`;
1.5 s of silence before a tone at 30 fps: `leading_silence_frames == 45` and `audio_leading_silence`
Info, a programme opening on a full-scale tone: `0`; a 5-frame range at 30 fps →
`MixLoudnessRangeTooShort { sample_frames: 8_000, required: 19_200 }` and nothing decoded; with
`profile: Some(Youtube1080p)` a −20 LUFS tone raises `audio_loudness_out_of_tolerance` (`observed
"-2000"`, `allowed "-1500..=-1300"`) and a −50 dBTP peak raises `audio_true_peak_over_ceiling`
Error; digital silence raises `audio_silent` alone, as Warning, with `technical_pass == true`;
`AudioQcProvenance::default()` equals the eight §2.4 strings; every leaf of the serialized report is
an integer, bool, string, or null; `evidence_only` is always `true`.

### 3.11 Part A media evidence map (brief b–h, j)

(b) §3.2. (c) §3.3 Tech 3341 cases 1–5. (d) §3.5 Tech 3342 cases 1–4 plus −60/−15. (e) §3.6.
(f) §3.4. (g) §3.1 chunking; §3.8 observer equality. (h) §3.9. (j) §3.10.

## 4. Part A agent and app

### 4.1 `get_audio_qc` (D8, Q3)

New `crates/kinewright-agent/src/audio_qc_tool.rs` mirroring `color_qc_tool.rs` (header :1-13,
description :30-39, args :85-127, handler :137-193, envelope :197-221, assumptions :224-259).
`AudioQcArgs` (`deny_unknown_fields`): `expected_revision: Option<TimelineRevision>`,
`start_frame`/`end_frame: Option<TimeCode>` (AU1's half-open window, server.rs:9306-9318),
`profile: Option<DeliveryProfile>`. Refusals: stale revision → the uniform `stale_revision`
(color_qc_tool.rs:142-146); `start_frame >= end_frame` → `error_text("get_audio_qc needs
start_frame < end_frame; got {start}..{end}")` (server.rs:7175-7178's idiom);
`MixLoudnessRangeTooShort` → `error_text("get_audio_qc needs at least one 400 ms gating block
(19200 sample frames); got {n}")`; any other measurement error → `error_text("could not measure
audio qc: {error}")`. Envelope `{ "timeline_revision", "evidence_only": true, "applied": false,
"report", "assumptions", "exceptions" }` — the CC6 shape minus `stage` and `full_resolution`, which
a mix measurement does not have. Text, hundredths throughout, `none` via
`render_optional_hundredths` (server.rs:10814-10816):

```text
audio_qc range={start}..{end} profile={id|none} target={t}±{tol} ceiling={c} technical_pass={bool}
master lufs={} momentary_max={} short_term_max={} lra={} true_peak={} peak={} frames={}
balance={} clipping L samples={} runs={} bp={} R samples={} runs={} bp={} leading_silence={} trailing_silence={}
exception {Severity} {code} {field} observed={} allowed={}                      (one per exception)
```

Assumptions, fixed order: (1) "Measured at 48 kHz stereo through the export mix path with the
document's buses, master chain, and pan law applied: the same signal export encodes." (2)
"Clipping is counted per channel on the summed master before the single clamp; loudness, true
peak, balance, and silence are measured after it. A clipped run is three or more consecutive
full-scale samples on one channel." (3) "Loudness follows ITU-R BS.1770-4 gating on complete
400 ms blocks and a range shorter than one block is refused; loudness range follows EBU Tech 3342;
true peak is an 8x oversampled measurement conformant to EBU Tech 3341 tolerances and reads at most
0.04 dB under between grid points." (4) with `profile`: "The target is the profile's published
delivery contract (get_delivery_profiles). Distance from it is evidence; normalization is a
separate, explicit export step, and monitoring is not delivery." (5) "Evidence only. This tool
constructs no operation, changes no document, and never gates an export: technical_pass is not
export_ready." Dispatch arm beside `get_color_qc` (server.rs:922-925); `get_` infers `Inspector`
(schema.rs:40-42), so no override entry. `get_delivery_profiles` (server.rs:3337-3373) adds
`"loudness_target": profile.loudness_target()` per profile (F12) and its description (server.rs:
10130) ends "…including exact raster, codecs, bitrates, and the loudness target normalization and QC
read" (F15).

### 4.2 Part A registry effect: 130 / 78 and the moved assertions (F23, F15)

| site | today | Part A |
| --- | --- | --- |
| schema.rs:16 `INSPECTOR_TOOL_NAMES: [&str; 77]` | 77 | **78**; `"get_audio_qc"` after `"get_audio_spectrum"` (:82) with the AU2-style comment |
| server.rs:19325 `INSPECTOR_TOOL_NAMES.len()` | 77 | 78 |
| server.rs:19333-19352 M36 description budget list | 11 names | + `"get_audio_qc"` |
| server.rs:9992 `get_audio_levels` description | "decoded silent, which is what a muted or solo-suppressed track reports" | clause kept; + the refusal sentence (F5, Q3) |
| server.rs:10130 `get_delivery_profiles` description | "raster, codecs, and bitrates" | + "and the loudness target …" (F15) |
| server.rs:21245-21260 `(serialized, served)`, `input_schema_bytes`, `description_bytes` | 1 421 520 / 5 660, 1 293 084, 107 271 | regenerated; the erratum records the split |
| server.rs:21262-21271 and tests/mcp_server.rs:2529-2533 served quad | 7 / 5 660 / 3 510 / 998 | **byte-identical** |
| tests/mcp_server.rs:2496-2523 | 129 tools, 77 inspectors | 130 / 78; messages name AU3; the name list gains `get_audio_qc` |
| server.rs:14765-14776, :19446-19448 (M34/M31 lists) | — | unchanged |
| M36-AGENT-RUNTIME-EFFICIENCY.md:111-112 | AU2 Part B rows | + Part A rows |

### 4.3 `BaselineProofAnalysis` forwards

`color_qc_ui.rs:431-465` gains one-line forwarding arms for `audio_qc` (Part A) and
`verify_delivery_audio` (Part B), with AU2's comment. No other double changes.

### 4.4 The Mixer `LOUDNESS` section (D10a, F12, F20, F11, F14)

`chain_pane` (mixer_pane_ui.rs:80-86) gains `snapshot: LoudnessSnapshot` and `target:
LoudnessTarget`, which moves its two callers (mixer_ui.rs:492, the harness at :2297) and
`measure_chain_pane` (:2281); `master_pane` (:130-142) calls `loudness_section(ui, snapshot,
target) -> bool` after `pane_title` and before `pan_law_rows`. `snapshot` is the app's last
`Playback::loudness()`, polled **every frame the Mixer is visible, playing or paused** — unlike
`mix_peaks`, which `mixer_panel` reads only `if self.playing` (mixer_ui.rs:423-427) — so the F15
frozen figures show. `target` is **always** `export_delivery_profile(self.export_dialog.
delivery_aspect).loudness_target()` (export_ui.rs:133-140; the `const fn` becomes `pub(crate)`) —
the dialog's current profile target, regardless of any checkbox (F12, F20). Geometry, top to
bottom, height budget **90 px** (80 before Part B; E38) pinned by a `measure_loudness_section` helper (F20; the dock is
320 default / 260 minimum, app.rs:1583-1587, and the pane scrolls, mixer_pane_ui.rs:96-101):

1. caps label `LOUDNESS` (`TEXT_MUTED`);
2. two rows `M` and `S`: a 16 px `MICRO` label, a bar `MIXER_LOUDNESS_BAR_HEIGHT = 6.0` px tall
   (new `theme.rs` token beside `MIXER_REDUCTION_METER_HEIGHT`) across the remaining width minus a
   56 px `MICRO` readout (`−18.3` or `—`); fill `loudness_bar_fill(lufs) = ((lufs + 4000) /
   4000).clamp(0, 1)` over **−40…0 LUFS**, `0` for `None`; colour `loudness_bar_color(lufs,
   target)`: `TEXT_SECONDARY` below `target − tol`, `STATUS_SUCCESS` inside `[t − tol, t + tol]`,
   `STATUS_WARNING` above; `None` draws no fill; never accent (DESIGN.md:59-69);
3. one `MICRO` line `I −16.0 LUFS · LRA 6.2 LU · TP −1.3 dBTP · 0:42`, `—` per `None`, `TP` in
   `STATUS_DANGER` above the ceiling;
4. a `Reset` small button → `playback.reset_loudness()`; no operation is pushed.

A muted `MICRO` sentence closes the section: in Part A *"monitoring is not delivery: playback is
never normalised"*; Part B appends *"; the export step normalises the file"* (F14, F21). Pure
`loudness_bar_fill`, `loudness_bar_color`, `loudness_readout(snapshot)` are unit-pinned (A18).

### 4.5 The master strip line

`master_strip` (mixer_ui.rs:712-745) adds one `MICRO` `TEXT_MUTED` line `I −16.0` (`I —` when
`None`) between `meters_and_fader` and `Edit`. `a_bus_and_master_strip_fit_the_mixer_dock`
(:2250-2279) gains the line and records the master figure (196 today; ≈ 210 expected) under
`BUDGET = 240.0`; DESIGN.md:337's "196 for the master" becomes the measured figure.

### 4.6 DESIGN.md (Part A)

The Mixer section (DESIGN.md:329-371) gains: *"The master pane opens with a `LOUDNESS` section:
momentary and short-term as horizontal bars over −40…0 LUFS, integrated, loudness range, and true
peak as micro readouts, and a `Reset` that restarts integration. The readouts describe what has
been heard, not what has been rendered ahead into the output ring. The bars carry a status colour
against the export dialog's current profile target — success inside the target's tolerance, warning
above it — and are `text-secondary` below it; quiet is never a failure. The master strip carries
the integrated figure as one micro line under the fader. Monitoring is not delivery: playback is
never normalised."*

---

# Part B — Delivery

---

## 5. Part B core model and media

### 5.1 `ExportSettings.loudness_normalization` (D6, F4)

`ExportSettings` (media.rs:1014-1041) gains `#[serde(default, skip_serializing_if =
"Option::is_none")] #[schemars(default)] pub loudness_normalization: Option<LoudnessTarget>` before
`cancellation`, documented "AU3 §5.6: when Some, the export brings the finished master to this
target before encoding. A job parameter: `export_settings()` leaves it None, so every existing call
site encodes byte-identically." Every struct literal gains `None` — **17 literals in 11 files**
(the critic's 22 counted five `-> ExportSettings {` signature lines):

| file | literals |
| --- | ---: |
| `kinewright-core/src/delivery.rs` (`export_settings`, :174) | 1 |
| `kinewright-media/src/export.rs` (`measurement_settings`, :1145) | 1 |
| `kinewright-media/src/engine.rs` (`timeline_loudness`, :776-786) | 1 |
| `kinewright-media/src/audio.rs` (:3180, `parity_settings` :3369) | 2 |
| `kinewright-media/src/verify.rs` (`verification_settings` :1909, :2325) | 2 |
| `kinewright-media/src/cc1_fixtures.rs` (:3932) | 1 |
| `kinewright-media/src/cc6_fixtures.rs` | 1 |
| `kinewright-media/src/media_matrix_tests.rs` (:872) | 1 |
| `kinewright-media/tests/generated_media.rs` (:512, :591, :680, :762, :856) | 5 |
| `kinewright-media/examples/m4_verify.rs` (:148; `clippy --all-targets` compiles it) | 1 |
| `kinewright-app/src/export_ui.rs` (:1039-1048) | 1 |

`cc6_core.rs` calls `export_settings()` and needs no change. CC6 §10 holds: no `Document` field,
and `cc6_export_settings_and_job_records_serialize_deterministically` (cc6_core.rs:1397-1440) is
unchanged because the absent key is skipped.

### 5.2 `ExportAudioReport`, `ExportReport`, `Export::export_document_reporting` (D6, F11, N4 Q2)

```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ExportAudioReport {
    pub target: LoudnessTarget,
    pub before: AudioLoudness,
    /// Equal to `before` when skipped. `after.true_peak_dbtp_hundredths` is the pre-encode true peak.
    pub after: AudioLoudness,
    /// Sum of the passes' gains; 0 when skipped.
    pub applied_gain_hundredths: i32,
    /// 0 (skipped), 1, or 2.
    pub limiter_passes: u8,
    /// max(0, before.true_peak + applied_gain − after.true_peak): how far the limiter pulled the
    /// true peak down, derived from the two meter readings, 0 when it was an identity or skipped.
    pub peak_reduction_hundredths: i32,
    /// `after.integrated` within `target ± tolerance` (F11: two misses still export; this says so).
    pub on_target: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")] #[schemars(default)]
    pub skipped_reason: Option<String>,
}
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ExportReport {
    #[serde(default, skip_serializing_if = "Option::is_none")] #[schemars(default)]
    pub audio: Option<ExportAudioReport>,
}
// Export (media.rs:2115-2143), defaulted so the seven export doubles (facts-app §4) need no change:
fn export_document_reporting(&self, document: Arc<Document>, out: &Path, settings: ExportSettings,
    progress: ProgressSink) -> Result<ExportReport, MediaError> {
    self.export_document(document, out, settings, progress).map(|()| ExportReport::default()) }
```

`FfmpegMediaEngine` overrides it (engine.rs:1234-1276 gains a third method); the internal
`export_document_with_luts` → `export_document_inner` → `export_to_temporary` chain returns
`ExportReport` and `export_document` discards it. `Export::export_document` keeps returning `()`.

### 5.3 `DeliveryAudioVerification` and `Analysis::verify_delivery_audio` (D7, F8/Q2, Q5, F3)

```rust
/// A measurement of the written file's audio. It never touches the file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DeliveryAudioVerification {
    pub output_path: std::path::PathBuf,
    pub measured: AudioLoudness,
    /// The measurement lane: always 48_000 / 2.
    pub sample_rate: u32,
    pub channels: u16,
    /// Decoded frames at 48 kHz; may differ from the source by AAC priming/padding (§5.8).
    pub sample_frames: u64,
    /// `settings.loudness_normalization` as the job ran (F8): Some only when normalization was asked for.
    pub target: Option<LoudnessTarget>,
    pub exceptions: Vec<AudioQcException>,
    pub technical_pass: bool,
}
// Analysis, after verify_delivery_output (media.rs:1755-1764):
fn verify_delivery_audio(&self, path: &Path, target: Option<LoudnessTarget>)
    -> Result<DeliveryAudioVerification, MediaError> { /* default NotImplemented */ }
```

With a target: `loudness_target_exceptions(.., "delivery")` — `delivery_true_peak_over_ceiling`
Error, `delivery_loudness_out_of_tolerance` Warning, `delivery_loudness_range_over_maximum` Warning
— plus `delivery_audio_silent` **Warning** (Q5) when `measured.integrated == None`; the
verification is not length-refused, so on a file shorter than 400 ms that reading would be
ambiguous — the §5.8 fixtures are ≥ 5 s and a real export shorter than one block is the operator's
own programme, stated in the exception message ("silent, or shorter than one 400 ms gating block").
With `None` the verification is a measurement: `exceptions: []`, `technical_pass: true` (F8). Both
workers pass the same value. `DeliveryVerification` (delivery.rs:1390-1410) is unchanged.

### 5.4 Part B contract tests

`au3_core.rs` items B1–B3; `contracts.rs` wire pins; `export_queue.rs` B4.

### 5.5 Stage order in `export_to_temporary` (D6), normative

```text
export.rs:168   let mut audio_mix = mix_audio(document, settings)?;                        unchanged
+               let audio = settings.loudness_normalization
+                   .map(|target| normalize_master(&mut audio_mix, target, settings)).transpose()?;
…               muxer, encoders, video loop, video drain (:170-345)                          unchanged
export.rs:347   encode_audio(&audio_mix, …)                                                unchanged
                Ok(ExportReport { audio })
```

With `None` the `map` is not entered, nothing is allocated, and `encode_audio` receives
`mix_audio`'s bytes exactly (B6).

### 5.6 `normalize_master` (D6, F10, F11, F5, Q3, F6)

The buffer entry Part B needs, in `audio.rs` beside `AudioEffectRuntime` (audio.rs:717, whose
`new` :735 and `process_frame` :850-856 stay module-private):

```rust
/// AU3 §5.6: run one static audio node over a whole interleaved buffer. Builds the runtime from
/// `effect`, calls `process_frame` with `project_at = TimeCode::ZERO` on every frame, feeds `Lh`
/// zero frames, drops the first `Lh` output frames, and returns `Err(MediaError::Backend)` — never
/// a panic (F11) — if the output length differs from the input's.
pub(crate) fn process_buffer_static(effect: &Effect, sample_rate: u32, channels: usize,
    input: &[f32]) -> Result<Vec<f32>, MediaError>;   // N4 F5's name; `Result` per F11's last bullet
```

1. `input.len() / 2 < LOUDNESS_GATING_BLOCK_FRAMES` → `skipped_reason: Some("shorter than one
   400 ms gating block")`, gain 0, passes 0, `after = before`, `on_target: false`; return (Q3).
2. `before = LoudnessMeter::measure(mix, 48_000, 2)?`; `before.integrated == None` → `skipped_reason:
   Some("silent")` (distinct from step 1, never conflated); return.
3. `gain = target.integrated − before.integrated`; outside **−6 000..=3 600** (the planner's guard,
   server.rs:8505-8509) → skipped with `"required gain {gain} hundredths exceeds -6000..=3600"`.
4. `g = 10f64.powf(f64::from(gain) / 2_000.0) as f32`; every sample `x *= g`.
5. **Limiter pass.** `ceiling_tenth_db = (target.true_peak_ceiling_dbtp_hundredths −
   LOSSY_CODEC_TRUE_PEAK_HEADROOM_HUNDREDTHS).div_euclid(10)` = **−30** for every §2.3 target
   (inside `−120..=0`, effect.rs:2056-2060). `process_buffer_static(&Effect { name:
   "audio_true_peak_limiter", parameters: { ceiling_tenth_db, lookahead_milliseconds: 5,
   release_milliseconds: 50, true_peak: 1 } }, 48_000, 2, &mix)?` — `Lh = stage_latency_frames(5,
   48_000) = 240` (AU2 §3.5: the node's total delay is exactly `Lh`, `D` inside it,
   audio.rs:694-698). `limiter_passes = 1`.
6. `after = measure`. If `after.integrated < target − tolerance`, `correction = target −
   after.integrated`, apply as in 4, limit again with a fresh runtime, `limiter_passes = 2`,
   re-measure. **At most two passes**; `applied_gain_hundredths = gain + correction`; `on_target =
   |after.integrated − target| ≤ tolerance`; `peak_reduction_hundredths = max(0, before.true_peak +
   applied_gain − after.true_peak)`. When two passes still miss, the export **succeeds**, the report
   says `on_target: false`, and the verification warns (F11).
7. No further clamp: `mix_pass`'s clamp ran before the step and the limiter's ceiling proof (AU2
   §3.5) bounds every emitted sample under `10^(−30/200) = 0.708`.

**Pins (B5, B7).** A −30 LUFS 1 kHz stereo tone → the streaming target: `gain == 1_600`, `passes ==
1`, the limited buffer **bit-identical** to the gained one (per-channel peak −14.0 dBFS under the
−3 dBTP ceiling: AU2 A27's identity; −11 would be the one-channel figure, F6),
`peak_reduction_hundredths == 0`, `after.integrated == −1400 ±5`, `on_target`; the same tone with a
0 dBFS impulse every 500 ms: `passes == 2`, within tolerance, `after.true_peak ≤ −300 + 5`,
`peak_reduction_hundredths > 0`; a 300 ms programme skips "shorter than one 400 ms gating block";
digital silence skips "silent"; a +40 dB request skips with the range reason; `process_buffer_static`
preserves length, passes an under-ceiling buffer bit-identically, and returns `Err` (not a panic)
when handed a node that would change the length; `None` → `report.audio == None` and `mix_audio`'s
bytes.

### 5.7 `verify_delivery_audio` decode path (D7, F18, F5)

`FfmpegMediaEngine::verify_delivery_audio` (beside `verify_delivery_output`, engine.rs:1031-1038,
in the same crate as the decoder): `probe_path(path, AssetId(0))` (decode.rs:44; verify.rs:820's
bare-path call) → `end_sample = frame_to_samples(asset.duration, 48_000, asset.fps)` (the
`asset_loudness` mapping, engine.rs:763-774) → `AudioDecoder::open(path, 48_000, 2, 0, end_sample)`
(audio.rs:2628, :2646; `AudioDecoder`, `open`, and `next_chunk` (:2697) become `pub(crate)`, as
N4 F5 rules; `decode_audio_range` stays `pub(crate)`) → `while let Some(chunk) =
decoder.next_chunk()? { meter.push(&chunk)? }` → `finish()`. **Streamed**: `decode_audio_range`
(audio.rs:39-68) is not used because it materialises the whole `Vec`. A file with no audio stream
returns `MediaError::Backend("{path} has no audio stream to verify")`, recorded as unavailable
(§6.2, §6.6). The result never moves, renames, or deletes the file.

### 5.8 Encoded fixtures and budgets (exit-gate clause 1; F1/N4 Q2, F18, F19)

New `crates/kinewright-media/tests/au3_fixtures.rs` under `test-util`, following cc6_fixtures.rs
(:712-760, :1027-1041, :1075-1085). Four generated programmes (`GeneratedMedia::ffmpeg`,
test_support.rs:53-58), each **12 s** (F18: ≥ 5 s) of `testsrc2` with the audio below at 48 kHz
stereo, encoded like the AU1 fixture (mcp_server.rs:2639-2656), exported through the real engine,
and verified with `verify_delivery_audio(path, Some(target))` on the written file:

| lane | programme | profile / target | role |
| --- | --- | --- | --- |
| **hot-noise** | `anoisesrc` pink noise at ≈ −20 LUFS (crest factor 12–14 dB) | `Youtube1080p` / −1400 | limiter works continuously at −3 dBTP |
| **hot-impulse** | the B5 programme: 1 kHz tone at −30 LUFS with a 0 dBFS impulse every 500 ms | `SourceMaster` / −2300 | limiter catches transients; AAC pre-echoes around them |
| **no-limiting** | 1 kHz sine at −20 dBFS + white noise at −30 dBFS (≈ −19.8 LUFS, summed peak −11.8 dBFS after gain, 8.8 dB under the ceiling) | `Youtube1080p` / −1400 | LU evidence only; **labelled "no limiting needed"; does not exercise the ceiling** |
| **failing direction** | the no-limiting programme with the setting `None` | verified against `Some(STREAMING_PLATFORM_TARGET)` | `\|integrated + 1400\| > 300`, `delivery_loudness_out_of_tolerance` raised |

**Budgets, one constant each, set before the fixtures land, never widened:**

```rust
/// Distance from the target's integrated value the decoded file may show.
const FIXTURE_LOUDNESS_BUDGET_HUNDREDTHS: i32 = 100;
/// dBTP the AAC encode may add over the pre-encode true peak (report.after.true_peak_dbtp_hundredths).
const FIXTURE_AAC_OVERSHOOT_BUDGET_HUNDREDTHS: i32 = 50;
const FIXTURE_MINIMUM_MARGIN: f64 = 2.0;                              // CC6 rule 11.0.5
```

**Assertions, every lane:** raw `|measured.integrated − target| ≤ 100` and margin `100 /
max(|deviation|, 1) ≥ 2.0`; `measured.true_peak ≤ −100`; `technical_pass`; `report.on_target`.
**Hot lanes additionally (F1, N4):** the limiter was at work — `report.limiter_passes ≥ 1`,
`report.peak_reduction_hundredths > 0`, and `report.after.true_peak_dbtp_hundredths ≥ (ceiling −
200) − 50` (the pre-encode peak sits at the limiter ceiling, so the lane cannot silently regress to
idle); `overshoot = measured.true_peak − report.after.true_peak ≤ 50` (the difference of two printed
numbers); and per F19 `margin_dbtp = ceiling − measured.true_peak ≥ 2 × 50`. The no-limiting lane
asserts `report.peak_reduction_hundredths == 0` and `limiter_passes == 1`. Every measurement is
printed (`AU3_FIXTURE_MEASURED lane=… profile=… integrated=… pre_encode_true_peak=…
decoded_true_peak=… overshoot=… gain=… passes=… reduction=… margin_lu=… margin_dbtp=…`); a zero
deviation prints `unbounded`. **Both operating systems.** One constant each; the Windows FFmpeg
package differs (CC6 Appendix A) and its measurement is not in hand when these constants are written
(CC6 §6.3 point 2). If Windows lands inside the raw budget but under 2× margin, the constant's doc
comment gains a per-OS **note** with the figure — never a `cfg`-conditioned constant.
`LOSSY_CODEC_TRUE_PEAK_HEADROOM_HUNDREDTHS` is re-baselined only from the two hot lanes' printed
`pre_encode_true_peak` / `decoded_true_peak` pairs on both OSes (F10).

**AAC priming and padding (F18).** The measurement is of the decoded stream as the demuxer presents
it: the decoder maps by PTS relative to the stream start (audio.rs:2662, :2792-2797) and reads no
`skip_samples` side data, so the 1 024-frame priming may appear as leading zeros and up to one AAC
frame (21.3 ms) of padding may trail. Neither is compensated; on a ≥ 5 s fixture the effect on
integrated is under 5 hundredths and on true peak none; no test pins decoded and source
`sample_frames` equal or any sample-exact alignment.

**Descendants.** AU5 §3.11's repair lanes
(`crates/kinewright-media/tests/au5_fixtures.rs`) are this section's house style carried forward:
exact `wav_f32` bytes rather than lavfi for the same reason §5.8 gives, one printed measurement line
per lane, a named budget constant carrying its derivation, a `FIXTURE_MINIMUM_MARGIN` of 2.0, and no
`cfg`-conditioned tolerance anywhere.

### 5.9 Part B media evidence map (brief a, i)

(a) §5.8. (i) §5.6; `export_document_reporting` on the engine returns the step's report.

## 6. Part B agent and app

### 6.1 `queue_export.normalize_loudness` (F9/Q6)

`QueueExportRequest` (export_queue.rs:46-70) and `QueueExportArgs` (server.rs:9013-9040) gain
`#[serde(default)] normalize_loudness: bool` (**default false**), documented *"AU3 §5.6: bring the
finished master to the profile's loudness target (get_delivery_profiles) under a true-peak limiter
before encoding. A job parameter, not a document edit; the source is untouched, so no confirmation
gate."* `WorkItem` (:260-277) carries it; `run_work_item` (:738-742) materialises settings, then
`settings.loudness_normalization = work.normalize_loudness.then(|| work.profile.loudness_target())`
before cloning `verification_settings`, and calls `exporter.export_document_reporting` (:755-759)
under the same `catch_unwind`. The field is added at `QueueExportRequest`'s destructuring pattern
(export_queue.rs:545) and three literals (:1960, :2155, server.rs:3615).

### 6.2 `ExportJobRecord` and the queue (F8, F9, R6, F4)

`ExportJobRecord` (export_queue.rs:95-161) gains, each `#[serde(default, skip_serializing_if =
"Option::is_none")]`: `audio_report: Option<ExportAudioReport>`, `audio_verification:
Option<DeliveryAudioVerification>`, `audio_verification_unavailable_reason: Option<String>`; its
one literal (`let record = ExportJobRecord {` in `enqueue`) gains the three `None`s. A pre-AU3
record deserialises with all three `None`; a record with them `None` serialises byte-identically.
After `verify_output` (:917-956) the worker **always** calls `verify_audio_output(state,
&output_path, verification_settings.loudness_normalization)` — independent of `verify`, which
governs the video comparison only — under `catch_unwind`; `mark_completed` (:958-1000) records the
outcome. **Verification never fails a job** (:958-963): a target miss leaves `Completed`, `error:
None`, `technical_pass == false`. A job cancelled after its encode carries
`EXPORT_CANCELLED_BEFORE_VERIFICATION` in the audio reason too. `AvailabilityAnalysis`
(:1193-1305) gains `AudioVerificationDouble { NotImplemented | Measured | Refused | Panics }`.

### 6.3 The planner on the true-peak limiter (D9, F10, F17)

`normalization_bus` (server.rs:8497-8577): the final `audio_limiter` (:8560-8568) becomes
`audio_true_peak_limiter { ceiling_tenth_db: ceiling_hundredths.div_euclid(10), lookahead_milliseconds:
5, release_milliseconds: 50, true_peak: 1 }`; the compressor (:8526-8540) gains no lookahead, so the
chain declares 5 of `CHAIN_LOOKAHEAD_MILLISECONDS`'s 20; `processing_ceiling` (:8430-8432) uses the
core `LOSSY_CODEC_TRUE_PEAK_HEADROOM_HUNDREDTHS` (−300 → `−30` tenths). `first_effect_id`
(:8395-8400) also scans `document.audio_mix.master.effects` **for symmetry** — ids are unique per
owner (AU2 N1), so this is not a correctness change. The tool description (server.rs:10070) reads
"…compressor/gain/true-peak-limiter processing…" and adds *"The plan's bus declares 5 ms of
lookahead, so a running playback stops and re-cues once when it is committed."* (F17b). The final
gate stays on the **sample** peak (:8466-8471; under a true-peak limiter sample peak ≤ true peak ≤
ceiling); `predicted.true_peak_dbtp_hundredths` appears in the structured output through
`AudioLoudness`. The `(legacy)` display label (inspector_ui.rs:3925-3930) **stays** for documents
that already carry `audio_limiter`; only the comment's reason changes (F17c).

**Direct tests (B11):** `normalization_context_refuses_each_malformed_argument` (the eight `Err`
strings, :8357-8391, verbatim); `normalization_bus_emits_a_true_peak_limiter_and_never_the_legacy_
clamp` (positive gain with and without compression, negative gain: node names and ids, the four
limiter parameters, `chain_lookahead_milliseconds == 5`); `first_effect_id_scans_the_master_chain`;
and `au3_plan_audio_normalization_converges_through_the_real_engine` in `tests/mcp_server.rs` on the
AU1 sine fixture: plan at −1600 / −100 / 100, commit, `get_audio_levels` master within ±100 of −1600
with `true_peak_dbtp_hundredths ≤ −100`. **Regression pin:** the g2/g3 windows
(kinewright-eval.rs:1441-1445, :1573-1577) and the suite self-test (:5255-5272) are unchanged and
re-run; F17's arithmetic (baselines ≥ 100 from the window edges) is recorded in the test doc.

### 6.4 `get_export_jobs` text and the Part B registry effect

`export_jobs` (server.rs:3643-3654) keeps its first line and appends, per job with an
`audio_verification`, `job {id} audio lufs={} true_peak={} lra={} target={t|none} technical_pass={}`,
and per job with an `audio_report`, `job {id} normalization gain={} passes={} reduction={}
on_target={} skipped={reason|none}`. Its description (:10153-10156) gains "…the decoded audio
loudness verification, and the normalization report". Registry: counts stay **130 / 78**;
`input_schema_bytes` moves by `QueueExportArgs`'s boolean only (no served tool embeds it,
mcp_server.rs:2529-2533); `description_bytes` moves for `plan_audio_normalization`, `queue_export`,
`get_export_jobs`; the served quad stays 7 / 5 660 / 3 510 / 998; server.rs:21245-21260 and M36 are
regenerated a second time.

### 6.5 The export dialog `Loudness` row (D10b, F21)

`ExportDialog` (export_ui.rs:37-66) gains `normalize_loudness: bool` (`false` at app.rs:346-359).
After the `Delivery depth` row (:1380-1396):

```text
Loudness   [ ] Normalize to {profile.as_str()} target ({decibels(t)} LUFS, {decibels(c)} dBTP)
           a job parameter, not a document edit · monitoring is not delivery: playback stays as mixed
```

`profile = export_delivery_profile(self.export_dialog.delivery_aspect)` (:133-140); `decibels`
(:485) renders two decimals (`-14.00`). `ConformanceKey` (:80-93) is unchanged. `start_export`
(:1039-1048) sets `loudness_normalization: self.export_dialog.normalize_loudness.then(||
profile.loudness_target())`; the worker calls `media.export_document_reporting` (:1080-1085) and,
after `worker_verification`, `worker_audio_verification(&result, &cancellation, ||
worker_analysis.verify_delivery_audio(&worker_output, verify_settings.loudness_normalization))` with
the same `catch_unwind`/cancel rules (:724-744).

### 6.6 `ExportOutcome`, `audio_verification_lines`, and the labels (F8, F20, F11)

`ExportOutcome` (:97) becomes a struct `{ path, result: Result<ExportReport, MediaError>,
verification: Option<ExportVerification>, audio_verification: Option<ExportAudioVerification> }` with
`ExportAudioVerification::{Measured(Box<DeliveryAudioVerification>), Unavailable(String)}`; the
`Disconnected` arm of `poll_export` (:1177-1181) builds the struct; `poll_export` (:1169-1209)
stores both and `report.audio` on the dialog; the status becomes `Exported {path} · {video label}
· {audio label}`, and `cc6_export_dialog_reports_the_verification_result` (:2289) is updated for
the appended label.

```rust
pub(crate) fn audio_verification_status(v: Option<&ExportAudioVerification>) -> VerificationStatus
// AUDIO NOT VERIFIED (STATUS_WARNING)   none or Unavailable
// AUDIO OVER CEILING (STATUS_DANGER)    an Error-severity exception (only the ceiling is one)
// AUDIO OFF TARGET   (STATUS_WARNING)   a Warning exception (tolerance, LRA, silent) and no Error
// AUDIO VERIFIED     (STATUS_SUCCESS)   target present, no exception
// AUDIO MEASURED     (TEXT_SECONDARY)   no target: a measurement, not a conformance claim (F8)
pub(crate) fn audio_verification_lines(v: Option<&ExportAudioVerification>,
    report: Option<&ExportAudioReport>, profile_target: LoudnessTarget) -> Vec<VerificationLine>
```

The audio status is its own line, not a fifth level of `verification_status` (F20; its four-label
comment at :443-450 stands). Lines after the status: `Unavailable` → the muted "The encode succeeded
and the file is untouched; audio verification could not run: {reason}"; else `integrated {x} LUFS ·
target {t} ±{tol} · within|OFF` (with no target: `integrated {x} LUFS · reference {t} ±{tol}
({profile}) · not normalized`), `true peak {x} dBTP · ceiling {c} · within|OVER` (or `· reference
{c}`), `loudness range {x} LU` (`—`), then from the report `normalization gain {±x.xx} dB · limiter
passes {n} · reduction {x.xx} dB · on target|off target` / `normalization skipped: {reason}` /
`normalization off`, then one line per exception in `severity_color`. `verification_block`
(:844-852) draws the video lines, a caps `AUDIO` sub-heading, then these; still uncapped.

### 6.7 `KINEWRIGHT_SCREENSHOT_SHOW=export` and the height budget

`app.rs:346-359` sets `export_dialog.open` to `matches!(var, Ok("export"))`; the recognised set
(app.rs:331-339) becomes `settings, timeline, transcript, mixer, mixer-chain, export`.
`EXPORT_DIALOG_MAX_BODY_HEIGHT = 420.0` (export_ui.rs:35) is unchanged: the body is a `ScrollArea`
(:1248-1250), the `Loudness` row adds one `CONTROL_HEIGHT` (26 px) plus one muted line to the
pre-verification controls, and the `AUDIO` sub-block adds at most nine lines to a block that already
scrolls. The implementer records the measured pre-verification body height in §0 (no `measure_*`
helper exists for the body, which is a `KinewrightApp` method; facts-app §1).

### 6.8 DESIGN.md (Part B)

Dialogs (DESIGN.md:379-384) gains: *"The export dialog's `Loudness` row is a checkbox naming the
profile's target in LUFS and dBTP; like the delivery depth it is a job parameter and never a
document edit, and its muted line says that monitoring is not delivery. After an export the
verification block carries an `AUDIO` sub-block whose own status line reads `AUDIO VERIFIED`,
`AUDIO OFF TARGET`, `AUDIO OVER CEILING`, `AUDIO MEASURED`, or `AUDIO NOT VERIFIED`; only the
ceiling is danger."* The Mixer sentence of §4.6 gains "; the export step normalises the file".

### 6.9 Known limits

- **Stereo-only measurement.** Weights declared, not applied; mono is measured as one channel
  (3 dB under its dual-mono reading); channels beyond two are ignored.
- **The live meter runs at the device rate, export at 48 kHz.** Coefficients and sub-block lengths
  are derived per rate; a 44.1 kHz device agrees with export to the bilinear warping's fraction of a
  decibel.
- **The live meter is fed up to one second ahead of the loudspeaker and published at the audible
  position**, so its figures lag the fill by the ring depth and agree with the sound; the filter
  state restarts at every pause→continue (a 4 ms transient in the next sub-block).
- **The live meter resets on seek and on play from a new position**, and continues through a pause
  at the same position (F15). The frozen figures on pause are the heard prefix's.
- **The true-peak meter carries a stated bias**: exact on its grid, at most 0.042 dB under between
  grid points, never over (§3.6).
- **A range shorter than one 400 ms gating block is refused** by `mix_levels` and `audio_qc`
  (`MixLoudnessRangeTooShort`); `short_term_max` is `None` under 3 s; the AU1 2.002 s fixture
  reports no short-term maximum. `timeline_loudness` and `verify_delivery_audio` are not refused
  and read `integrated == None` on a sub-block programme.
- **Normalization takes at most one corrective pass**; a second miss exports, reports `on_target:
  false`, and the verification warns.
- **AAC overshoot is held by headroom, not measured before encode**; the decoded true peak of the
  two hot lanes is the fixtures' evidence, and priming/padding is not aligned sample-exactly (§5.8).
- **`measure_mix_spectrum` holds whole stems** and `mix_pass` holds one decoded buffer per track on
  every path (R2). **Each `normalize_master` limiter pass transiently holds a second master
  buffer** (`limit_to_delivery_ceiling` returns a fresh `Vec` of `len + Lh·channels` and the old one
  is freed on assignment), so the step peaks at about twice the master's footprint on top of
  `mix_pass`'s per-track buffers; two passes do this sequentially, so the peak is 2×, not 3×.
- **Conformance is by Tech 3341/3342 tolerance, not the ITU table** (§3.6); the limiter's detector
  is a different, unchanged 4× filter.
- **The planner's gate is the sample peak** its argument names; the true peak is reported beside it.
- **Silence is floored to project frames** from 10 ms windows; a silence under 10 ms reads 0.
- **Monitoring is not delivery.** The operator hears the un-normalized master; the file is delivered
  at the target. Stated in the pane, the dialog, and DESIGN.md (F21).

## 7. Acceptance checklists

### Part A (A1–A19)

Discharges exit-gate clause 2 verbatim from ROADMAP-AND-WORKFLOWS.md:647 — *"the QC report is
integer-reported and evidence-only"* — and the row's measurement deliverables, *"Momentary,
short-term, integrated, LRA, and true-peak metering; per-profile loudness targets; audio QC
(`get_audio_qc`) with clipping, silence, channel-balance, and target checks"* (§1.2).

A1. `AudioLoudness` with the four new fields `None` serialises to the pre-AU3 five-key string and
    the pre-AU3 JSON deserialises with them `None`; still `Copy + Eq`; `AudioQcRequest` `{}`
    round-trips as the empty object and a windowed one as
    `{"range":{"start":30,"end":60},"profile":"youtube_1080p"}` (au1_core.rs:400-418's template);
    `MediaError::MixLoudnessRangeTooShort` renders its message and has no recovery code — core
    `tests/contracts.rs`, `tests/au3_core.rs`.
A2. **Per-profile loudness targets** (deliverable): `loudness_target()` is `const` and returns
    −2300/100/−100/None for `SourceMaster` and −1400/100/−100/None otherwise; the two consts differ;
    `LOSSY_CODEC_TRUE_PEAK_HEADROOM_HUNDREDTHS == 200` and the agent has no second copy (a grep
    test on `server.rs`) — core `tests/au3_core.rs`, agent `server.rs`.
A3. `audio_qc_exceptions` raises exactly the §2.4 table with its `field`/`observed`/`allowed`
    strings, sorted; `audio_silent` is a Warning that leaves `technical_pass` true and suppresses
    the target, balance, and silence entries; Δ `None` with `J ≠ ∅` and one silent side raises
    `audio_channel_imbalance` and Δ `None` with `J = ∅` raises nothing; the six consts have the
    §2.4 values — core `tests/au3_core.rs`.
A4. `no_audible_media` is raised with the second message for a muted-only and a solo-silenced
    timeline, with the first for a media-free one, and not for an audible one; Info;
    `Playback::loudness()`, `reset_loudness()`, and `Analysis::audio_qc` defaults;
    `AudioQcProvenance::default()` equals the eight §2.4 strings — core `tests/au3_core.rs`,
    `media.rs`.
A5. `finish()` is bit-identical for one push, 1 024-frame pushes, and 7-frame pushes; a misaligned
    push returns `Err` and consumes nothing; `measure_loudness == LoudnessMeter::measure`;
    audio.rs:4192's `assert_eq!` still holds — media `loudness.rs`, `audio.rs`.
A6. K-weighting reproduces Tables 1–2 at 48 kHz and N1's 44.1/96 kHz values within 1e-6; the 997 Hz
    calibration reads exactly 0 / 0 / −2 at 48 / 44.1 / 96 kHz, −301 one-channel, and −2300 ±10 at
    `0.0708` on both channels — media `loudness.rs`.
A7. **Integrated metering** (deliverable): Tech 3341 cases 1–5 at 1 kHz read −2300 / −3300 / −2300
    / −2300 / −2300 ±10; 399 ms → `None`, 400 ms → `Some`, both with sample and true peak `Some` —
    media `loudness.rs`.
A8. **Momentary and short-term metering** (deliverable): Tech 3341 cases 12 and 9 read −2300 ±10
    on every complete window; the step pins at `j = 12/13` and `38/39`; series lengths — media
    `loudness.rs`.
A9. **LRA metering** (deliverable): Tech 3342 cases 1–4 read 1 000 / 500 / 2 000 / 1 500 ±10 (case 4
    ungated 3 000); the −60/−15 case reads 176 ±10 and 4 500 ungated; one gated window → `None` —
    media `loudness.rs`.
A10. **True-peak metering** (deliverable): §3.6's cases 15–18, 997 Hz, and 20 kHz at 0.5 within
     +20/−40; impulses exactly 0; the phase-sweep floor −5; noise within 2 of the 16× reference with
     the bias printed; `h[128] == 1.0` and `|h[8k]| < 1e-15` for `k ≠ 16`; raw sums within 2e-6 of
     1 — media `loudness.rs`.
A11. Observer equality per §3.8; `measure_mix_levels_trims_every_stem_family_to_the_requested_range`
     with its range widened to `0..4` and `sample_frames == 19_200`; a one-frame range returns
     `MixLoudnessRangeTooShort` before decoding; `mix_audio` bit-identical on
     `parity_document_with_master_chain`; the split-chunk window feeds exactly `T` frames — media
     `audio.rs`, `export.rs`.
A12. Live meter per §3.9 through `fill_ring` and the publish step with a simulated clock: heard-prefix
     convergence; the published value never leads the clock; pause after N seconds then continue at
     `paused_at` counts the pre-rendered second exactly once; `momentary == None` while paused;
     reset on seek, on play elsewhere, and through `Playback::reset_loudness` →
     `Control::ResetLoudness`; `programme_seconds` from the published key; `short_term == None`
     under 3 s and `integrated == None` under 400 ms in the live shape; the six atomics are stored
     once per publish; `AudioRuntime::new`/`fill` and the three `fill_ring` test callers thread the
     meter — media `audio.rs`, `engine.rs`.
A13. `audio_qc` per §3.10: per-channel clipping on a +6 dB full-scale tone, `Δ == None` on L-only,
     `Δ == 600 ±5` on −20/−26, 45 leading frames at 30 fps and 0 on a hot open, a 5-frame range
     refused with `{ sample_frames: 8_000, required: 19_200 }` and nothing decoded, the target
     exceptions, digital silence as `audio_silent` alone, a window straddling a chunk boundary
     counted once — media `audio.rs`.
A14. **The QC report is integer-reported** (exit-gate clause 2): every leaf of the serialized
     `AudioQcReport` and of `get_audio_qc`'s envelope is an integer, bool, string, or null (CC6's
     walk over `ColorQcReport`) — agent `tests/mcp_server.rs`
     (`au3_get_audio_qc_is_evidence_only_and_revision_gated`, after the AU2 pair at :2993).
A15. **The QC report is evidence-only** (exit-gate clause 2): `evidence_only == true`, `applied ==
     false`, the assumptions end with "technical_pass is not export_ready", the revision is
     unchanged, the stale-revision envelope, the inverted-range and short-range refusal texts, the
     four text lines of §4.1 on a known fixture, and `get_delivery_profiles` publishes
     `loudness_target` for all four profiles with the amended description — agent
     `tests/mcp_server.rs`.
A16. Registry: 78 inspectors with `get_audio_qc` after `get_audio_spectrum`; 130 / 52; the
     description under 1 024 B; `get_audio_levels`' gloss keeps its mute/solo clause and names the
     refusal; served quad byte-identical; figures regenerated; every §4.2 site moved — agent
     `schema.rs`, `server.rs`, `tests/mcp_server.rs`.
A17. `BaselineProofAnalysis` forwards `audio_qc` — app `color_qc_ui.rs`.
A18. `loudness_bar_fill` (−4000 → 0, −2000 → 0.5, 0 → 1, `None` → 0, clamped);
     `loudness_bar_color`'s truth table against the dialog's profile target; `loudness_readout`
     renders `—` per `None`; `chain_pane` and `measure_chain_pane` take the snapshot and target; the
     master pane paints `LOUDNESS`, `M`, `S`, `Reset`, and the Part A F21 sentence;
     `measure_loudness_section ≤ 90` (80 before Part B; E38); the snapshot is polled while paused; the master strip paints
     `I —` and `a_bus_and_master_strip_fit_the_mixer_dock` records it under 240 — app `mixer_ui.rs`,
     `mixer_pane_ui.rs`.
A19. `MIXER_LOUDNESS_BAR_HEIGHT` resolves; DESIGN.md contains the §4.6 sentences and the measured
     master figure; `MEDIA-POLICY.md` carries the observer/audible-position paragraph;
     `CHANGELOG.md`, the AU1/AU2 pointer sentences, the dsp.rs:307-312 amendment, and the M36 Part A
     row are present (string pins in the docs test) — app `theme.rs`, docs.

### Part B (B1–B16)

Closes exit-gate clause 1 verbatim — *"Encoded fixtures land within pinned LU/dBTP budgets on both
CI operating systems"* — and the row's remaining deliverables, *"normalization as an explicit
export step"* and *"decoded verification of the written file's loudness and true peak"* (§1.2).

B1. `ExportSettings` with `loudness_normalization: None` serialises without the key, byte-identically
    twice (cc6_core.rs:1397-1440's template); `Some` round-trips; `export_settings()` leaves `None`
    for four profiles × two depths; the pre-AU3 settings JSON deserialises — core `tests/au3_core.rs`.
B2. `ExportReport::default()` is `{}`; `ExportAudioReport` round-trips with and without
    `skipped_reason`; `export_document_reporting` defaults to `export_document` +
    `ExportReport::default()` on a counting double — core `tests/au3_core.rs`.
B3. `DeliveryAudioVerification` round-trips; `loudness_target_exceptions(.., "delivery")` emits the
    three `delivery_*` codes and `delivery_audio_silent` as Warning with its two-reading message, and
    an empty list with no target; `verify_delivery_audio` defaults to `NotImplemented` — core
    `tests/au3_core.rs`.
B4. Pre-AU3 `ExportJobRecord` and `QueueExportRequest` JSON deserialise (`normalize_loudness ==
    false`, three audio fields `None`) and serialise byte-identically when `None`; the destructuring
    pattern at export_queue.rs:545, the three request literals, and the one record literal carry the
    fields — agent `export_queue.rs`, `server.rs`.
B5. **Normalization as an explicit export step** (deliverable): §5.6's pins — 1 600 / one pass /
    bit-identical / reduction 0, two passes and reduction > 0 on the impulse programme, the three
    distinct skips, `on_target`, `applied_gain` as the sum — media `export.rs` or `audio.rs`
    (AU2 E19).
B6. Off is byte-identical: `None` → `report.audio == None` and `mix_audio`'s bytes; the CC6 fixtures
    pass unchanged — media.
B7. `process_buffer_static` drops exactly 240 frames, preserves length, passes an under-ceiling
    buffer bit-identically, returns `Err` on a length mismatch, and is handed `−30` tenths for every
    §2.3 target — media `audio.rs`, `export.rs`.
B8. **Decoded verification of the written file** (deliverable): on a generated AAC of a −20 LUFS
    tone, integrated −2000 ±30 and true peak within 50 of the source's, streamed through
    `AudioDecoder::next_chunk`; a video-only file refuses with §5.7's message; a −50 dBTP file
    against a −100 ceiling raises `delivery_true_peak_over_ceiling` Error; `None` target → no
    exceptions, `technical_pass` — media `au3_fixtures.rs`.
B9. `run_work_item` sets the setting from `normalize_loudness`, calls `export_document_reporting`,
    records `audio_report`, and runs the audio verification with `verify: false` as well as `true`
    passing `settings.loudness_normalization`; the four doubles land on the record; a miss never
    fails the job; cancel-after-encode fills both reason fields — agent `export_queue.rs`.
B10. `queue_export` accepts `normalize_loudness` (default false); `get_export_jobs` renders §6.4's
     lines; `get_delivery_profiles` targets equal `loudness_target()` — agent `tests/mcp_server.rs`
     (`au3_queue_export_normalizes_and_verifies_audio`, `WritingExporter` + `AvailabilityAnalysis`).
B11. Planner: §6.3's three direct tests and the real-engine convergence test; no emitted chain
     contains `audio_limiter`; `chain_lookahead_milliseconds == 5`; the description carries the
     re-cue sentence; g2/g3 specs and self-test unchanged — agent `server.rs`, `tests/mcp_server.rs`,
     `kinewright-eval.rs`.
B12. **Encoded fixtures land within pinned LU/dBTP budgets on both CI operating systems** (exit-gate
     clause 1): §5.8's four lanes — the two hot lanes pass raw and at ≥ 2× margin (`margin_dbtp ≥
     100`, `overshoot ≤ 50`) **with the limiter proven at work** (`limiter_passes ≥ 1`,
     `peak_reduction_hundredths > 0`, `after.true_peak ≥ −350`), the no-limiting lane passes LU with
     `peak_reduction_hundredths == 0`, the failing direction misses by > 300 and warns; every
     measurement printed with `pre_encode_true_peak` beside `decoded_true_peak`; the budgets are the
     two constants; green on Linux and Windows CI — media `au3_fixtures.rs`.
B13. Registry Part B: 130 / 78 unchanged; `input_schema_bytes` moves by `QueueExportArgs`'s boolean
     only; the served quad byte-identical; M36 row — agent `server.rs`, `tests/mcp_server.rs`.
B14. Dialog: the checkbox writes `Some(profile target)` / `None` into `start_export`'s settings; the
     label follows the aspect; the muted line contains "monitoring is not delivery";
     `ConformanceKey` unchanged; `ExportOutcome` carries the audio verification and report to
     `poll_export` including the `Disconnected` arm; `KINEWRIGHT_SCREENSHOT_SHOW=export` opens the
     dialog — app `export_ui.rs`, `app.rs`.
B15. `audio_verification_status` yields the five labels for none / Unavailable / Error / Warning
     only / no target; `audio_verification_lines` renders §6.6's lines including `normalization
     gain +6.40 dB · limiter passes 1 · reduction 0.00 dB · on target`, the reference form with no
     target, the skipped and off forms; a sub-decibel value keeps its sign (export_ui.rs:2674's
     case); `cc6_export_dialog_reports_the_verification_result` reads the appended status; the
     headless `verification_block` paints `AUDIO` — app `export_ui.rs`.
B16. DESIGN.md contains §6.8's sentences and the Part B suffix of §4.6's; README's bullets name
     loudness normalization and decoded audio verification; M34:127 is amended; the ROADMAP status
     paragraph names the two parts; `CHANGELOG.md` and the M36 Part B row are present — docs.

Exit gate for each part: its checklist green in `cargo test --workspace` locally on Linux, `cargo
fmt --check` and `cargo clippy --workspace --all-targets -- -D warnings` clean, both CI operating
systems green after push, and both review passes recorded. Part B additionally needs one hands-on
export of a real project with the checkbox on and off, both `AUDIO` blocks read by Riel. Part A
lands as "feat: complete AU3a — loudness measurement"; Part B as "feat: complete AU3b — loudness
delivery".

## 8. Files

**Part A.** Core: new `audio_qc.rs`; `delivery.rs` (`LoudnessTarget`, three consts,
`loudness_target`); `media.rs` (`AudioLoudness` fields, `LoudnessSnapshot`, `Playback::loudness`/
`reset_loudness`, `Analysis::audio_qc`, `MediaError::MixLoudnessRangeTooShort`); `qa.rs`; `lib.rs`;
`tests/contracts.rs`; new `tests/au3_core.rs`. Media: `loudness.rs` (rewritten; `push -> Result`,
`last_block_end`, `truncate_to`); `export.rs` (`MixObserver`, `NoObserver`, `FamilyWindow`,
`MixCollect`, `mix_pass`, `LevelsObserver`, `QcObserver`, `measure_audio_qc`, the length refusal in
`measure_mix_levels`); `audio.rs` (`fill_ring`, `AudioRuntime::new`/`fill` meter parameters, the
three `fill_ring` test callers at :3844/:3865/:3884, the A16 test's range); `engine.rs`
(`LiveLoudness`, the worker meter and 16-entry ring, `paused_at`, the tick publish,
`Control::ResetLoudness`, `loudness`, `reset_loudness`, `audio_qc`); `dsp.rs` (doc); `lib.rs`;
tests. Agent: new `audio_qc_tool.rs`; `schema.rs`; `server.rs` (dispatch, `delivery_profiles` and
its description, `get_audio_levels` gloss, `audio_levels` refusal text, headroom const removed,
figures); `tests/mcp_server.rs`. App: `color_qc_ui.rs`; `mixer_ui.rs` (`chain_pane` callers,
`measure_chain_pane`, polling, strip line); `mixer_pane_ui.rs` (`chain_pane` signature,
`loudness_section` and the three pure fns); `export_ui.rs` (`export_delivery_profile` visibility);
`theme.rs`; `app.rs`. Docs: this file, `CHANGELOG.md`, `MEDIA-POLICY.md`, `DESIGN.md`,
`AU1-MANUAL-MIX.md`, `AU2-EQ-AND-DYNAMICS.md` (pointers), `M36-AGENT-RUNTIME-EFFICIENCY.md`.

**Part B.** Core: `media.rs` (`ExportSettings` field, `ExportAudioReport`, `ExportReport`,
`Export::export_document_reporting`, `DeliveryAudioVerification`, `Analysis::verify_delivery_audio`);
`delivery.rs` (literal); `lib.rs`; `tests/contracts.rs`, `tests/au3_core.rs`. Media: `export.rs`
(`normalize_master`, stage order, literal); `audio.rs` (`process_buffer_static`; `AudioDecoder`,
`open`, `next_chunk` to `pub(crate)`; two literals); `engine.rs` (`export_document_reporting`,
`verify_delivery_audio`, literal); `verify.rs` (two literals); `cc1_fixtures.rs`,
`cc6_fixtures.rs`, `media_matrix_tests.rs` (one literal each); `tests/generated_media.rs` (five);
`examples/m4_verify.rs` (one); new `tests/au3_fixtures.rs`. Agent: `export_queue.rs` (request,
record, work item, `verify_audio_output`, the destructuring pattern and two request literals, the
record literal, doubles, tests); `server.rs` (`QueueExportArgs`, the request literal at :3615,
planner, `export_jobs`, descriptions, figures); `tests/mcp_server.rs`; `kinewright-eval.rs` (re-run
only). App: `export_ui.rs` (field, row, `start_export`, worker, `ExportOutcome` and the
`Disconnected` arm, `audio_verification_*`, block, the CC6 dialog test); `app.rs`; `color_qc_ui.rs`;
`mixer_pane_ui.rs` (the Part B sentence suffix); `inspector_ui.rs` (comment). Docs: this file,
`README.md`, `CHANGELOG.md`, `ROADMAP-AND-WORKFLOWS.md`, `DESIGN.md`,
`M34-CREATOR-DELIVERY-VERIFICATION.md`, `M36-AGENT-RUNTIME-EFFICIENCY.md`.
