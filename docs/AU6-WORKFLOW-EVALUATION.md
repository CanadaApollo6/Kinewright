# AU6 — Workflow evaluation

Status: revision 2 — for promotion
Depends on: [AU1 manual mix](AU1-MANUAL-MIX.md), [AU2 EQ and dynamics](AU2-EQ-AND-DYNAMICS.md), [AU3 loudness and delivery](AU3-LOUDNESS-AND-DELIVERY.md), [AU4 clip envelopes and automation](AU4-CLIP-ENVELOPES-AND-AUTOMATION.md), [AU5 repair and room tone](AU5-REPAIR-AND-ROOM-TONE.md), [CC7 workflow evaluation](CC7-WORKFLOW-EVALUATION.md), [M34 creator delivery verification](M34-CREATOR-DELIVERY-VERIFICATION.md), [M35 finished-cut benchmark](M35-FINISHED-CUT-BENCHMARK.md), [M36 agent runtime efficiency](M36-AGENT-RUNTIME-EFFICIENCY.md), [M39 dialogue pacing](M39-DIALOGUE-PACING.md), [M40 generalization gauntlet](M40-GENERALIZATION-GAUNTLET.md)
Scope: **proving that five named audio workflows complete end to end over the AU1–AU5 surface — by a person, by a scripted agent, and by a model — with every objective claim discharged by an ordinary `cargo test` on both CI operating systems, and with the human reviewer left only the balance and intelligibility questions the roadmap's audio row asks.**

AU6 adds **no MCP tool, no `Operation` variant, no effect descriptor**; the served surface stays **7 / 5 660 / 3 510 / 998** and the ledger **138 / 54 / 84 / 1 540 264 / 1 397 156 / 120 458**. (`pub enum Operation` carries **57** variants; **54** is `operation_tools().len()`, the generated-mutator count `crates/kinewright-agent/tests/mcp_server.rs:2555-2562` pins. The two numbers are different quantities and this contract writes both down once so a later reader does not re-derive it.) Every measured quantity comes from a core function, an `Analysis` method, or a tool response that exists at `11a6098`. AU1's mix model, AU2's chain semantics, AU3's loudness targets and delivery verification, AU4's automation model, and AU5's repair nodes and room-tone store are preserved verbatim: AU6 *consumes* them and records the margins. AU6 never widens an AU3 or AU5 budget, never re-baselines a codec tolerance, and never renames or deletes an existing test.

**The one product-change exception (N1 Q5/Q6/S1/S2, as amended by N2 Q1).** AU6 may close a person-path gap **when the evaluation shows the app's own text directs the person to a dead end**, provided the fix is **≤ ~450 lines, app-only**, carries its own tests and its own `docs/DESIGN.md` paragraph, adds no `Operation`, no tool and no descriptor, and moves no ledger byte. Exactly **two** changes qualify and no others: the track `AUTOMATION` target (§6.2, estimated **300–450** lines) and `changed_project_range`'s blindness to mix edits (§6.3, ≈ 40 lines). Everything else the evaluation finds is **recorded in §13 with a cost and an owner, not fixed**.

The words **must**, **must not**, and **may** in this document are normative.

---

## 0. Change log

### 0.1 Changes from the brief

`design-brief.md` D1–D9 was reviewed by `critic-brief.md` (B1–B9, S1–S13, nits 1–12, Q1–Q12) and ruled on by `orchestrator-notes.md` **N1**, which is binding and supersedes the brief wherever they differ. Each line below is a design change carried from the brief into draft v1; §0.2 carries what the probe and the contract critic then changed.

- **N1/Q1/B9 — the scenario geometry is fixed.** The brief fixed no fps, raster, programme length, sample rate, turn ranges, or window/hop, and the probe could not start. `au6_scenarios` pins `AU6_SOURCE_FPS = 25`, `AU6_SOURCE_WIDTH/HEIGHT = 320/180`, 48 kHz stereo, `AU6_PROGRAMME_FRAMES = 300`, one authored turn pattern for every voice scenario, and `mix_window_levels` window = hop = 200 ms = exactly 5 project frames (§2.3).
- **N1/Q2/B8 — (e) becomes two eval tasks.** `EvalDefinition.deliverable` is one `Option<EvalDeliverableSpec>` carrying one profile and `produce_deliverable` writes one `finished.<ext>` per run, so five tasks could not deliver two encodes. The suite has **six** tasks — `a1..a4`, `a5a`, `a5b` (§7.2).
- **N1/Q3/B4 — (c)'s 31 learned bands are not gated by document equality.** A data-and-arithmetic core module may not obtain an expected value by calling the function under test. *(N2 Q2 has since withdrawn N1's analytic array as the replacement gate — see §0.2 item 19.)*
- **N1/B3 — (c)'s levels are re-derived downward from the silence detector.** The brief's noise + hum bed sat at ≈ −29 dBFS, above `DEFAULT_SILENCE_THRESHOLD_DBFS_HUNDREDTHS = -3_500` (`crates/kinewright-media/src/derived.rs:33`), so **no authored gap would ever be detected as silence** and both the planner and the app's `Learn profile` would refuse by name. Voice −24 dBFS, noise −42 dBFS, hum −45 dBFS total.
- **N1/Q4/B5 — (b) uses one bus per voice.** `MixSpectrumPoint::Track` is the post-track-stage, **pre-bus-chain** stem (`crates/kinewright-core/src/media.rs:1242-1253`), so a track point can never show a bus chain's effect, and one shared bus sums both voices. (b) authors "Voice A" and "Voice B"; the chain sits on Voice B and gate (2) reads `Bus(Voice B)`.
- **N1/Q5/Q6/S1/S2 — AU6 ships the track automation target.** The brief scored (a)'s ducking leg with a clip-envelope substitute; the app's own `AUTOMATED_FADER_TOOLTIP` points a track-fader user at a pane that cannot help. Part A ships the fix and (a)'s person path becomes the identical document (§6.2).
- **N1/S3(a) — `changed_project_range` learns about mix edits.** Curve operations imply the keyframe span; scalar mix operations imply the whole programme range (§6.3). **S3(b)** — `RemoveAudioBus` and bus-rename controls — is §13 with its half-day cost.
- **N1/Q7/S7 — room tone is in scope for (c).** Three of the five audio planners and the one audio Action had no coverage at all in the brief. (c) now splits its dialogue clip, removes an interior range, captures room tone and fills the gap **before** the repair chain is applied; `plan_audio_normalization` and `plan_clip_fades` are exercised in (b). All five planners and the one Action are thereby exercised.
- **N1/Q8/S4/S5 — (d) is re-cut around one product claim.** `SyncGroup` is **base-document state**, not a canonical operation, so (d) is not person-N/A; `MasterTrackUntouched` is dropped in favour of the existing `ProgramAudioContinuous`; the master stem's **bit-identity** across the cuts becomes the load-bearing gate.
- **N1/Q9/S6 — `is_audio_assertion` is added beside `is_color_assertion`**, in the same exhaustive `const fn` shape, with `measurements` gated on the disjunction; CC7's pin is untouched. `NoClippingInQc` becomes `AudioQcTechnicalPass`.
- **N1/B1/B2 — the harness work is larger than D5 said.** `EvalDeliverableSpec` has no serde derives, so `normalize_to_profile_target` is a plain field edited at every literal; and D5 omitted the audio evidence block, without which six of the seven assertions are unreachable (§7.6).
- **N1/Q10/S10 — Windows numbers are recorded, never gated**, with no manifest field; Appendix A carries them after the first green run.
- **N1/Q11/S13 — the agent crate is budgeted separately**, one shared `FfmpegMediaEngine` in the media lane, a `Drop` guard on every raw temp file.
- **N1/Q12/B6/S11 — the questions are single-clause and the leak needles cannot contain one.** Every brief question joined two clauses with "and", which one `Option<bool>` cannot answer; and the brief's needle set contained words its own questions used, so the leak test would have failed by construction.
- **N1/B7 — (e)'s person leg is a builder-level identity**, because `KinewrightApp::start_export` builds `ExportSettings` inline with hard-coded bitrates and is not constructible in a test.
- **N1/S9 — the margin rule is restated in margin terms**: floors `measured/budget ≥ 2`, ceilings `budget/measured ≥ 2`, zero terms "infinite (measured exactly zero)".
- **N1 nits 10–12 — §13 gains five live items and one re-deferral**, including **AU5 R30**, which is structurally undischargeable in AU6 and is re-deferred to Riel's hands-on session by name.

### 0.2 Changes from the probe and the critic

The probe (`target/review/au6/probe-report.md`, 825 lines, run 2026-09-10 at `11a6098`, debug lane, working tree restored) measured every bracketed number in draft v1 and escalated **E1–E12**; the contract critic (`target/review/au6/critic-contract.md`, 508 lines) returned **11 blockers, 21 substantive findings and 14 nits**, resolving 78 of the draft's ~162 `path:line` citations. `orchestrator-notes.md` **N1.5** adopts every budget in the probe's §0 table as the constant's value and rules **A1–A15**; **N2** rules on the critic and is binding. **Every critic finding is accepted as written unless N2 amends it.** Each line below is a design change, and every replaced number, name or claim is given with its **was**.

1. **A1 + S5 + N2 Q8 — the encoding lanes are 8 s, (d)'s delivery leg is cut, and the lane budgets triple.** The probe measured the AU6 media lane's floor at **≈ 105 s**, not the brief's "target ≤ 45 s"; 90 s was reachable only by cutting (d)'s delivery leg, so D8's "may be cut" item was load-bearing, not optional. All four BS.1770 quantities stay populated at 8 s (§5.1 of the probe), which saves 7.1 s per normalized encode and 0.48 s per verification. `AU6_ENCODE_PROGRAMME_SECONDS = 8` (200 project frames); **(d)(5) the delivery leg is CUT** and (d) is gated on the bit-identical master stem, the silent scratch tracks and the mix gates, with (e) proving delivery at both targets; `AU6_MEDIA_LANE_BUDGET_SECONDS = 180` against **≈ 90 s** measured and `AU6_AGENT_LANE_BUDGET_SECONDS = 120` against **≈ 60 s**. *Was:* 12 s encodes, (d)(5) present and merely "cuttable", `AU6_MEDIA_LANE_BUDGET_SECONDS_LINUX = 90` / `AU6_AGENT_LANE_BUDGET_SECONDS_LINUX = 90`. **The call pattern is now normative** because `mix_levels` costs 8.9–33× `mix_window_levels` over the same range (8 829 ms against 266–990 ms whole-programme): `mix_levels` **once per authored turn** (one report carries every track and bus), everything RMS from **one whole-programme `mix_window_levels` render per mix point** sliced by integer window index, and **`audio_qc`, not `mix_levels`, for LRA**.
2. **A2 + E2 — the 1 s gap is required, not preferred, and the contract states the arithmetic.** With `plan_audio_ducking`'s shipped defaults (attack 150 + hold 200 + release 400 ms = 750 ms) a 1 s gap leaves **8 un-ducked project frames = exactly one 200 ms window**, and a 500 ms gap leaves **zero**. The gate-(2) gap population is that single named window. *Was:* the gap length was stated without its reason and the brief's original 500 ms bracket would have measured ≈ 0 depth.
3. **A3 + E3 — the track trims are measured constants, and the nominal dBFS difference is named as the wrong model.** The two (a) speakers are 6.00 dB apart in RMS dBFS but only **3.65 LU** apart in LUFS, because K-weighting lifts the 2 kHz band ≈ 3.1 dB and the 1 kHz band ≈ 0.8 dB; a trim derived from the authored dBFS (`+60` tenth dB) over-corrects by 2.35 LU and makes gate (3) **worse than no trim at all** (235 against 365). `AU6_INTERVIEW_A2_TRIM_TENTH_DB = 37`, `AU6_PODCAST_A_TRIM_TENTH_DB = 72`, `AU6_PODCAST_B_TRIM_TENTH_DB = -72`, each with its K-weighting derivation in its doc comment. *Was:* `AU6_A_TRIM_A_TENTH_DB` / `AU6_A_TRIM_B_TENTH_DB` / `AU6_B_TRIM_*`, all `[probe]`, with no statement that the nominal model is wrong.
4. **A4 + E4 — (c)'s SNR bracket is withdrawn and replaced by a floor under the silence threshold.** A 10 dB SNR at a −24 dBFS voice puts the gap floor at −34 dBFS, i.e. **above** the −35.00 dBFS detector threshold, and the learn range would never be found: the brief's bracket was infeasible. The rule is **"the gap floor sits at least 5 dB below −35.00 dBFS"** — `AU6_LEARN_GAP_BELOW_SILENCE_MIN_HUNDREDTHS = 250` (floor), measured **526**, margin 2.10× — and the nominal SNR is **reported** (16.36 dB nominal, 20.41 dB as the percentile estimator reads it), never authored. *Was:* `[SNR 10 dB]` as an authored quantity and `AU6_C_GAP_SILENCE_MARGIN_DB_HUNDREDTHS` as the constant name.
5. **A5 + E5 — (b)'s speaker B is a 1 Hz level ride, not 4 Hz syllabic, and the chain is pinned.** A 200 ms window integrates 0.8 of a 4 Hz cycle and averages the modulation away: bypass spread measured **377** at 4 Hz against **1 482** at 1 Hz. (b) authors a **1 Hz log-domain level ride, 18 dB depth, trough at both ends**, and the 200 ms grid stays. Canonical chain **c1m**: HPF 80 Hz; compressor threshold −400, ratio 400, attack 5 ms, release 50 ms, knee 60, makeup **+130**, RMS detector, RMS window 10 ms — every value a named constant. The makeup gain is not cosmetic: without it the compressed bus drops 12.5 LU below the untouched Voice A bus and **both** the voice-match and LRA gates fail. *Was:* "deep syllabic dynamics [AM depth 18 dB]" at the voice rate, and `AU6_B_COMPRESSOR_*` all `[probe]`.
6. **A6 + E6 — (b)'s gated clips are authored inside a turn, both edges on envelope maxima.** `plan_clip_fades` skips any clip whose head **or** tail window is silent, so its first attempt proposed **nothing**. (b)'s gated clips are `90..103` and `103..120` on track 2, **no gated clip starts at frame 0**, and the canonical fades are **1 frame in / 1 frame out per clip**, which is what the planner proposes. *Was:* "clips authored with a hard mid-utterance trim edge" and `[probe]` fades.
7. **A7 + S7 + E7 — no gate asserts the absence of `mains_hum_present`, and the 50 Hz term is dropped as a gate.** The 60 Hz cascade's sixth-octave shoulders reshape the spectrum around 50/100/150 Hz enough that `hum_50_excess_db_hundredths` rises from **0 to 829** (planner chain) or **1 072** (media chain) and a second `mains_hum_present` finding appears *after* repair. A gate asserting "the 50 Hz twin is unmoved" **cannot pass**. Selectivity is proven instead with `hum_60_harmonic_excess_db_hundredths` per harmonic (`crates/kinewright-core/src/audio_repair.rs:125`), each ≥ `AU6_HUM_HARMONIC_DROP_MIN_DB_HUNDREDTHS` (**500**, closed by item 49), with the rule that an unpopulated reading under one whole Goertzel block is a **failure**, not a skip. The manufactured 50 Hz finding is recorded in §13 as an AU5 limit with a cost. *Was:* §4(c)(2) asserted the 50 Hz twin **unmoved**.
8. **A8 + E8 — `plan_audio_normalization` is a document-state planner and is orthogonal to (e).** Its arguments are `{ track_ids, target_lufs_hundredths, maximum_sample_peak_dbfs_hundredths, tolerance_hundredths }` (`crates/kinewright-agent/src/server.rs:11720-11733`) — **there is no `profile` argument**; passing one is refused `-32602 "missing field 'track_ids'"`. It builds **one deterministic delivery bus** (document state), while (e)'s normalization is `ExportSettings.loudness_normalization` (job state); the two reach the same integrated number by different mechanisms and the contract says so. In (b) it is called at **both** targets and its `predicted.integrated` asserted within `AU6_NORMALIZATION_PREDICTION_MAX_LU_HUNDREDTHS = 25` (ceiling, measured **1**, margin 25×); its single `UpsertAudioBus` is an **evidence call only** and is **not** part of (b)'s canonical document. *Was:* "called and its response asserted (shape + proposal)", with the profile-driven reading D3 implied.
9. **A9 + E9 + B4(i) — `mix_audio_stems` stays `pub(crate)` and the AU6 media fixtures live in `src/au6_fixtures.rs`.** Confirmed rather than changed, and the two stem-identity pins (curve-owner equivalence, master-stem survival) live there. *Was:* stated, but with no ruling that the public surface would not be widened instead.
10. **A10 + E10 — canonical `SplitClip` sequences are ordered late-to-early.** Splitting clip 1 at 75 and then at 150 fails `OperationFailed { op_number: 2, error: SplitOutsideClip { clip: ClipId(1), at: TimeCode(150) } }`, because the second half becomes a *new* clip. §2.6 states it and the app builder test asserts it. *Was:* (c) and (d) listed their splits in ascending frame order.
11. **A11 + E11 — every AU6 audio asset is re-stamped onto the 25 fps project grid.** An audio-only WAV probes at `Rational::default()` = **30 fps**, so `validate()` refuses with `IncorrectDocumentDuration` unless `au6_sources` stamps each asset's `fps`/`duration` onto the project grid. AU5 dodged this by making its project 30 fps; the 25 fps geometry cannot. `validate()` is called after **every** canonical batch step. *Was:* unstated.
12. **A12 + E12 — anything derived from a detector is pinned as committed output, with the authored range beside it.** `timeline_silences` opens each window ≈ 1 frame early and closes it ≈ 4 frames late: the authored click-free gap `275..312` is committed as **`274..312`**, and `plan_audio_ducking`'s detected windows are `{1,54}, {77,129}, {151,204}, {227,279}` against authored turns `0..50, 75..125, 150..200, 225..275`. The authority publishes both. *Was:* `AU6_C_LEARN_GAP = TimeCode(275)..TimeCode(312)` as the pinned range, and `AU6_A_DUCK_KEYFRAMES` described as transcribed from the planner's arithmetic without saying the frames are detector output.
13. **A13 — the three ducking curve owners are bit-identical, and the gate reads `Bus(Music)`.** `SetTrackAutomation`, the Music bus's own `gain_curve` and the bed clip's `audio_gain_curve` produce a **bit-identical master and a bit-identical bus stem** (`max |Δ| = 0.000e0`); only the *track* stem differs, and only for the bus route. Gate (2) therefore reads `MixSpectrumPoint::Bus(Music)`, which reads 890 for all three owners while `Track(A3)` reads **−1** for the bus route. The equivalence is recorded as **evidence** with an `assert_eq!` on the `f32` vectors, and with the track AUTOMATION target shipped the person path is the track curve. *Was:* the alternatives were listed in §6.5 as untested prose.
14. **A14 — the mux recipe is FFV1 mkv + `pcm_s16le`, and audio-only export is accepted.** All three of `pcm_s16le` / `pcm_f32le` / `pcm_s24le` mux, probe as `AudioVideo` and export through the real path; `pcm_s16le` is canonical (it is what AU3's `lane_media` already writes) and the other two are noted. Separately, a document with a single `TrackKind::Audio` track and **no picture at all** exports successfully — the brief's open question is answered **"not refused"**. *Was:* `-c:a` was `[probe]` and the risk section treated a mux failure as a live hazard.
15. **A15 — the reachable typed-code set is fixed.** Asserted: `capture_room_tone`'s `revision_conflict` and `capture_refused`, plus the input-validation codes reachable with bad arguments (`unknown_asset`, `invalid_source_range`, `room_tone_capture_too_long`, `project_not_saved`); and `stale_revision` on `get_audio_qc` / `get_audio_repair`. The **four planners' prose refusals are asserted by exact string** and listed in §13 as the typed-envelope debt. *Was:* seven `capture_room_tone` codes including `core_rejected`, which draft v1 had already escalated as unreachable.
16. **B1 + N2 Q1 — the track route is `MixerSelection::Track(TrackId)` with `chain(self) -> Option<AudioChain>`.** `MixerSelection` is `Bus | Master` with two exhaustive `impl` methods (`crates/kinewright-app/src/mixer_ui.rs:279-281`, `:286-291`, `:299-304`), and `AudioChain` is a **core** enum (`crates/kinewright-core/src/media.rs:1171-1174`) used by engine telemetry, the `Learn profile` seam and core's own automation validator. AU6 does **not** add `AudioChain::Track`: `chain()` returns `Option<AudioChain>` and the four call sites handle `None`, so the change stays **app-only** and core is untouched. N1's size rule is amended to **≤ ~450 lines**, and the estimate is restated **300–450**. *Was:* "reuses the existing `edit_toggle` with `MixerSelection::Track(id)`" with no statement that `MixerSelection` cannot answer `chain()`, and "≈ 150–250 lines".
17. **B2 — `AutomationTarget` gains one variant, and the draft's own sentence was self-contradictory.** It gains `TrackParameter(&'static str)`, with the two-way mapping **`Fader ↔ "gain_tenth_db"`** and **`TrackParameter ↔ "pan_percent"`** onto `SetTrackAutomation.parameter`, whose schema is closed to exactly those two strings (`crates/kinewright-core/src/operation.rs:120-131`, `TRACK_AUTOMATION_PARAMETERS`, `crates/kinewright-core/src/model.rs:565`), and three new arms in `automation_targets`, `automation_target_label` and `set_automation_curve`. N1 forbade a new `Operation`, tool or descriptor — not a new app enum variant — so the fix is legal; the draft's wording was what was wrong. *Was:* "`AutomationTarget` gains no variant: … spelled `AutomationTarget::TrackParameter(&'static str)`".
18. **B3 — two person-path builders become `pub(crate)` and `AU6_TEST_SOURCES` grows to eleven.** `clip_audio_operation` (`crates/kinewright-app/src/inspector_ui.rs:4503`) and `noise_profile_operation` (`crates/kinewright-app/src/app.rs:2647`) are **private**, so (b)'s and (c)'s tests could not have lived in `mixer_ui.rs`'s `mod tests` as §11.2 placed them. Both become `pub(crate)` as an **R60-range erratum**, the tests stay in `mixer_ui.rs`, and `AU6_TEST_SOURCES` grows from nine to **eleven**, naming `inspector_ui.rs` and `app.rs`. *Was:* nine entries and a builder table that named two unreachable functions.
19. **B4 + S4 + N2 Q2 — the analytic 31-band array is withdrawn as a gate and replaced by two claims.** The app cannot be called from another crate (`kinewright-app` is a binary with no `[lib]`) and its "profile writer" only echoes the 31 values it was handed, so `au6_c_the_two_profile_writers_agree` was **not constructible and vacuous either way** — it is **deleted**. And the analytic derivation was under-specified by three whole conversions: the reduction is the **20th percentile over ten 4 096-frame windows** (`NOISE_PROFILE_PERCENT = 20`, `crates/kinewright-media/src/spectrum.rs:52-54`), `band_level_hundredths` integrates with **half-bin overlap** so the eleven bands from 20–200 Hz collect ≈ one whole bin rather than their nominal width (`spectrum.rs:443-471`), and `profile_band_tenth_db` **floors toward −∞ and clamps into `-1200..=0`** (`crates/kinewright-media/src/export.rs:1815-1834`). Replaced by: **(i)** a **regression pin, labelled as such** — `AU6_C_LEARNED_PROFILE_TENTH_DB: [i32; 31]`, transcribed from the probe's learned profile, asserted as an **exact `assert_eq!`** (item 49: drift measured 0, so no drift constant is declared), worst band printed; and **(ii)** one **analytic one-sided bound** that *is* derivable — every learned band ≤ the mean-power band level of the authored noise + hum partials (bin-count integrator, half-bin overlap included) plus `AU6_PROFILE_LEAKAGE_ALLOWANCE_TENTH_DB` (**60**, item 49), because a 20th percentile sits below the mean by construction. The closed-form percentile derivation is a §13 item with a cost. The app instead asserts that `noise_profile_operation` writes all 31 rows it is given into the named `audio_denoise` node (`write_noise_profile`, `app.rs:2672`). *Was:* `AU6_C_EXPECTED_PROFILE_TENTH_DB` as a two-sided analytic gate under `AU6_PROFILE_BAND_MAX_ERROR_TENTH_DB`, plus the deleted twin.
20. **B5 — `start_isolated_with_exporter_and_project_path` exists, and the draft's paragraph was false.** `McpServer::start_isolated_with_exporter_and_project_path(core, playback, analysis, exporter, project_path)` is `crates/kinewright-agent/src/server.rs:424-438`, and `set_project_path` / `project_path_handle` are `:533` / `:528`. The paragraph claiming the two requirements "never meet in one AU6 test" is **deleted**; `au6_a3` chooses freely, and the eval runner already gets both through `apply_fixture_project_path` (`crates/kinewright-agent/src/eval.rs:1507-1511`). *Was:* a load-bearing false statement an implementer would have trusted.
21. **B6 + N2 Q3 — (d)'s angle sources are video-only and (d)'s tracks are sync-locked.** `crates/kinewright-media/src/export.rs` contains **zero** `TrackKind` references — the mix does not filter by track kind — so an angle clip whose asset carried scratch audio would contribute it to the mix regardless of the `mute` on A1/A2, defeating gate (2). The angle `.mkv`s use the **`cc7_source` recipe with no `-c:a`**, and the scratch audio is separate `.wav` assets on A1/A2. Separately, `RippleDeleteClip` closes the gap on that clip's track **and every other sync-locked track** (`crates/kinewright-core/src/operation.rs:218-227`), and `Track.sync_lock` defaults **false**, so `au6_d_a_ripple_delete_shifts_the_master_stem` **could not have failed**: (d)'s base document sets `sync_lock: true` on **every** track (V1, V2, A1, A2, A3). *Was:* "two picture-carrying `.mkv` angle sources", a rule 7 asserting both encoding scenarios probe as `AudioVideo`, and a base document that never set `sync_lock`.
22. **B7 + N2 Q7 — `AudioEvalEvidence.window_levels` is keyed by a label, and two assertions are repointed.** `MixSpectrumPoint` derives no `Ord`/`PartialOrd`/`Hash` (`crates/kinewright-core/src/media.rs:1242-1253`), so it **cannot key a `BTreeMap`** and the struct did not compile; and `DialogueOverBedAtLeast` / `VoicesMatchedWithin` read `window_levels` while §4 measures those same terms with `mix_levels`, which is the S8 instrument violation §10.2 forbids. The map is `BTreeMap<&'static str, MixWindowLevelReport>` keyed by point label (`"master"`, `"bus:dialogue"`, `"bus:music"`, `"bus:voice_b"`, `"track:<n>"`), **no core derive is added**, and both assertions read `level_reports`. *Was:* `BTreeMap<MixSpectrumPoint, MixWindowLevelReport>` and two assertions on the wrong instrument.
23. **B8 + N2 Q4 — four constants are renamed, re-united or re-directed, before any number was pasted into them.** `AU6_HUM_DROP_MIN_TENTH_DB` → **`AU6_HUM_DROP_MIN_DB_HUNDREDTHS`** (the field `hum_60_excess_db_hundredths` is hundredths, `crates/kinewright-core/src/audio_repair.rs:117`); `AU6_REPAIR_SPEECH_LOSS_MAX_TENTH_DB` → **`AU6_REPAIR_SPEECH_LOSS_MAX_DB_HUNDREDTHS`** (`MixWindowLevelReport.windows` is dBFS hundredths, `crates/kinewright-core/src/media.rs:1381-1385`); `AU6_DECLICK_RESIDUAL_MAX_DB_HUNDREDTHS` (a ceiling on a residual) → **`AU6_DECLICK_ERROR_DROP_MIN_TENTH_DB`**, a **floor on an error drop in tenth dB**, which is the quantity AU5's instrument actually gates (`DECLICK_ERROR_DROP_BUDGET_TENTH_DB: f64 = 300.0`, `crates/kinewright-media/src/audio.rs:11004`, `:11445`); and `AU6_ROOM_TONE_SEAM_MAX_MILLIONTHS` → the seam is a **`MeasuredZero`** claim with **no budget and no probe placeholder**, because `assert_seam` asserts `measured <= ROOM_TONE_SEAM_BUDGET` **and** `measured <= 0.0` (`crates/kinewright-media/src/au5b_fixtures.rs:243-258`) and there is no "millionths" unit anywhere. AU6 writes its own `assert_au6_seam` helper rather than reusing one that hard-codes AU5's budget. **Drafter note on the two unit restatements:** N1.5 A7 and N2 Q4 both carry the pair "510 → 250" and "10 → 30", which are the **tenth-dB** figures; restated in the unit the field publishes they are budget **2 500** against measured **5 108** (2.04×) and budget **300** against measured **101** (2.97×). The physical budgets and the margins are unchanged; the arithmetic is recorded here so the restatement is visible rather than silent. **Superseded by item 49**, which closes the hum constant at 2 400 against 4 970.
24. **B9 + N2 Q5 — the duck-depth populations are named, and the plan's arguments are pinned.** With attack 150 / hold 200 / release 400 ms against a 200 ms grid, the first window of every turn is an attack ramp and the first windows of every gap are a release ramp, so a naive `min(gap) − max(speech)` draws both ends of the same ramp and collapses toward zero. The populations are: **speech = the whole windows from the second window of each turn to its end; gap = the final whole window of each 1 s gap** (windows 1–4 are ramp or ducked, window 5 is un-ducked — A2's arithmetic). `au6_scenarios` **pins the plan arguments as constants equal to today's defaults** — `AU6_A_DUCK_ATTACK_MS = 150`, `AU6_A_DUCK_HOLD_MS = 200`, `AU6_A_DUCK_RELEASE_MS = 400`, `AU6_A_DUCK_DEPTH_TENTH_DB = -120` — so a default change is caught rather than silently absorbed. **The gap-window rule here is superseded by item 46 (window 4, not window 5).** *Was:* "the 10 whole windows inside each turn" and "the 5 inside each gap", with the defaults relied on but not pinned.
25. **B10 — the `ExportSettings` comparison is field-by-field with `cancellation` excluded, and the difference set was wrong.** `ExportCancellation`'s `PartialEq` is `Arc::ptr_eq` (`crates/kinewright-core/src/media.rs:1402-1406`), so two independently built settings are **never** equal. And `delivery_color` is `delivery_color_for_depth(document, depth)` — the same document at the same depth for both jobs — so it is **identical**, not different. The real difference set is **`resolution`, `video_bitrate`, `audio_bitrate`, `loudness_normalization`**. *Was:* a whole-struct "differ only in …" comparison naming `delivery_color` among the differences.
26. **B11 + N2 Q6 — (c) has four revision advances, three of them commits, and waits for silence twice.** `capture_room_tone` applies its own `AddAsset` through `apply_operation_analyzing` (`crates/kinewright-agent/src/server.rs:2320-2324`) at `args.expected_revision` — it is **not** committed through `prepare_edit_plan`/`commit_edit_plan` — so the sequence is **commit(gap) → capture (its own advance) → commit(fill) → commit(repair)**, and batch 2 is the fill's `AddClip`s **only**. And `dialogue_repair_learn_span` refuses while **any referenced asset** is not `Ready | NoAudio` (`server.rs:9752-9774`), and the fill introduces a *new* referenced asset, so the script waits for `SilenceStatus::Ready` a **second** time after the fill commit. *Was:* three commits, one wait, and `au6_c_room_tone_operations(asset)` as a single committable batch containing the `AddAsset`.
27. **S1 — `EvalAssertion` has 57 variants, so the disjointness twin covers 63.** *Was:* 58 and 64.
28. **S2 — `AU6_BUDGETS` is sized to §4.1's row count, with the shared-constant rows named.** One row reuses a constant already counted (row 4) and five carry no constant at all (rows 8, 13, 16, 18, 19; N4 S6 corrected the count), so a 16-long array could not hold the table. *Was:* `[Au6Budget; 16]`.
29. **S3 — two budgets §3.2 gates on join §2.8 and §4.1.** `AU6_AUTHORED_LEVEL_TOLERANCE_HUNDREDTHS` (ceiling) and `AU6_VOICE_BAND_SEPARATION_BANDS` (floor) were thresholds with margin directions, no §2.8 entry and no §4.1 row, while §2.8 claims every gated threshold is one of its constants and §11.3 asserts a key per constant. *Was:* both bracketed inline in §3.2 only.
30. **S6 — the track curve coalesces on `track_automation_coalesce_key`.** Core already publishes `track_automation_coalesce_key(track, parameter)` (`crates/kinewright-core/src/model.rs:579`) for exactly this owner, beside `envelope_coalesce_key` (`:573`); using the mix key would fold a curve edit and a fader drag into one undo entry, the opposite of AU4's rule. *Was:* `track_mix_coalesce_key(track)` (`crates/kinewright-app/src/mixer_ui.rs:1396`) for both.
31. **S8 — the three `queue_export` script cells are completed and the temp-path rule is named.** `QueueExportArgs` (`crates/kinewright-agent/src/server.rs:11891-11924`) requires `expected_revision` and `output_path`; the cells sent neither, so a request written from the text would have failed deserialization. The output directory is built from **process id + thread id**, AU3's precedent (`crates/kinewright-agent/tests/mcp_server.rs:5246-5253`), and removed at the end. *Was:* `queue_export{profile: …, normalize_loudness: true}`.
32. **S9 — §11.2 names every test it inventories.** The arrays are pinned `[&str; N]` compared as sorted-set equality in both directions, so "the six §5.2 scripts" and "the five §6.1 tests" left eleven names to be reconstructed from two other sections, and `au6_d_a_cut_master_track_is_not_continuous` appeared nowhere. *Was:* items 37 and 38 named no test.
33. **S10 — `AU6_FORBIDDEN_HELPERS` goes back to three needles and `cfg(target_os` is asserted separately.** `uses_outside_prose` matches `needle(` or `("needle")`, and a real occurrence is `#[cfg(target_os = "windows")]`, which contains neither — the fourth needle was inert. *Was:* `[&str; 4]` including `"cfg(target_os"`.
34. **S11 — the DESIGN.md sentence §6.2 falsifies is corrected in the same edit, and the pin test has three loops.** `docs/DESIGN.md:456-457` ends "*the mixer is where a bus or master ride is edited*"; after §6.2 a **track** ride is edited there too, so the clause becomes "**a track, bus or master ride**". `the_design_note_states_the_new_mixer_rules` (`crates/kinewright-app/src/mixer_ui.rs:4267`) has **three** `for expected in [..]` loops — six phrases, four tokens, and an AU5 §6.5 rule-133 set — all three inventoried. *Was:* an appended paragraph, a falsified sentence left standing, and "six existing phrase pins and four token pins".
35. **S12 — the AU2 `remove_audio_bus` destructive-annotation asymmetry returns to §13.** It is in `plan_preview`'s destructive list (`crates/kinewright-agent/src/runtime.rs:365`) but absent from `destructiveHint` (`crates/kinewright-agent/src/schema.rs:410-413`), and the asymmetry is pinned by a test. Named in D6 and N0.6(i), it had dropped out of §13's list. *Was:* absent.
36. **S13 + N2 Q9 — bare task ids leave the leak-needle set.** `a1`..`a4` are valid **two-character lowercase hex**, and the scanned surface is a listing of `<12 lowercase hex>.png|.mp4` names plus a form carrying those same `blind_id`s — so the needles contradicted the set's own "≥ 3 digits" rule and would have matched hash prefixes. Task ids are scanned **only** as `"task_id": "aN"` / `"task_id": "a5a"` / `"task_id": "a5b"` forms; `"-sample-"` stays via `CC7_MACHINE_PROVENANCE_NEEDLES`. *Was:* bare `a1`, `a2`, `a3`, `a4`, `a5a`, `a5b` as needles.
37. **S14 — non-vacuity fixture 3 measures per authored voice **asset** and is scoped to (c).** (a)'s music bed is continuous at ≈ −20 dBFS over the whole programme, so any mix-point or master reading in a gap is ≈ −20, not below −35; and only an asset-domain reading is what `timeline_silences` cares about (`dialogue_repair_learn_span` walks `silence_status(asset)`, `crates/kinewright-agent/src/server.rs:9752-9758`). *Was:* "for (a), (b) and (c), the RMS of every authored gap window", which is false for (a).
38. **S15 — `capture_room_tone` takes SOURCE frames, and the authority publishes them.** The arguments are `source_start_frame` / `source_end_frame` on `asset_id`, and the doc is explicit that it decodes the chosen **source** range (`crates/kinewright-agent/src/server.rs:2103`, `:2155-2160`, `CaptureRoomToneArgs` `:11856-11873`). `au6_scenarios` publishes `AU6_C_LEARN_SOURCE_RANGE` beside the project range, with the surviving right-hand clip's `source_range` and the project→source identity stated. *Was:* the project range `AU6_C_LEARN_GAP` handed straight to the Action after two splits.
39. **S16 — two margin helpers, one per direction, in `src/au6_fixtures.rs`.** `margin(observed, allowed)` (`crates/kinewright-media/tests/au3_fixtures.rs:243-250`) computes `allowed / observed`, a **ceiling** margin only, and ten of §4.1's rows are floors; it is also in a `tests/` integration file that `src/au6_fixtures.rs` cannot reach. *Was:* "AU6 reuses that shape".
40. **S17 — `au6_c_gap_operations(next_clip_id: ClipId)` takes the id core will allocate.** `<right>` and `<middle>` are produced by the first split, and §2.1 forbids the module from touching a `Document`, so the two ids are pinned as `AU6_C_RIGHT_CLIP_ID` / `AU6_C_MIDDLE_CLIP_ID` with a core test proving core's allocation rule produces them. *Was:* `pub fn au6_c_gap_operations() -> Vec<Operation>` with no argument and no pinned ids.
41. **S18 — the spec field splits into `carries_picture` and `delivers`.** (a) owns a picture (for (e)'s encode) while advertising `encodes: false`; (e) reuses (a)'s buffers *and* picture. (a) is `carries_picture: true, delivers: false`; (e) is both true; **(d) is `carries_picture: true, delivers: false`**, because A1 cut its delivery leg. *Was:* one `encodes: bool` documented as "(d) and (e) only".
42. **S19 — §14 and Appendix A are rewritten against the probe report that now exists.** Both asserted "no probe has run" and "`probe-report.md` does not exist"; it exists, with E1–E12. §14 now carries the probe's measured cost model and its wall-clock projection, and Appendix A carries the probe's provenance and its measured table. *Was:* a checkable falsehood in both sections.
43. **S20 — v6's `acceptance_target` has six keys and the sixth is `note`.** *Was:* "the six keys v6 uses … and the dimension list".
44. **S21 — the runner snippet keeps `ungated_color_measurements`.** The gate today is `filter_map(color_measurement) … .extend(ungated_color_measurements(outcome))` (`crates/kinewright-agent/src/eval.rs:3638-3643`); a snippet showing only the `filter_map` half would have deleted the ungated colour measurements. *Was:* the `extend` line omitted.
45. **Nits N1–N14, all applied.** `PROFILE_BAND_NEUTRAL_TENTH_DB` is `crates/kinewright-core/src/effect.rs:1438` (**was** `:1439`, blank); `click_count` is `crates/kinewright-core/src/audio_repair.rs:126` (**was** `:128`, which is `click_density_per_minute`); `SessionConfig.max_turns` is assigned at `crates/kinewright-agent/src/eval.rs:1402` (**was** `:878`, blank) and `budgets.max_turns` is scored at `:3469` and `:4671-4672` (**was** `:2303`); the deliverable skip is `eval.rs:2766-2768` (**was** `:2769-2771`, the base-task-id extraction); the whitespace-token task-id rule lives in `human_review_template_with_questions` (`eval.rs:2769-2773`), not in `filter_definitions` (`crates/kinewright-agent/src/bin/kinewright-eval.rs:260`, which filters by `--only`); the capture's apply call starts at `server.rs:2320` (**was** `:2323`); `track_mix_coalesce_key` is `crates/kinewright-app/src/mixer_ui.rs:1396` (**was** `:1397`, the `format!` body); `review_questions` keys by **scenario index** (`kinewright-eval.rs:379`) and AU6's six tasks come from five scenarios, so §8.2 states what `HumanQuestion.id` is for `a5a`/`a5b`; the clipping fixture is renamed `au6_every_scenario_mix_does_not_clip` (**was** `au6_a_the_mix_does_not_clip`, whose `a_` prefix read as scenario (a)); the two AU6 ledger paragraphs are appended **after the AU5 Part B paragraph** (**was** "at `:2514`", which is inside that sentence); §2.3 states (c)'s eighth-gap override once, where the turn table is declared; §11.2 names the fixture that asserts the two `performance` budgets equal their code constants; and implementer **M**'s file set is unconditional (**was** `export_ui.rs` "conditionally").

**Amendments from probe-2 (N2.5 A16–A19).** Probe-2 (`target/review/au6/probe2-report.md`, 450 lines, same commit and lane) measured the five quantities N2's last bullet named and escalated **P2-E1..E3**; N2.5 rules them binding.

46. **A16 (P2-E1) — the duck-gap population is window 4, not window 5.** N2 Q5 counted `attack 150 + hold 200 + release 400 = 750 ms` and placed all of it at the gap's **head**. In fact hold + release (600 ms) sit at the head and the **next turn's attack** (150 ms) sits at the **tail**, so the fully-parked interval runs 600–850 ms and only the **600–800 ms window — window 4 — lies wholly inside it**; window 5 contains the attack ramp and reads **1.80 dB under parked**. Measured depth **1 187** at window 4 against **1 011** at window 5, both against the unchanged budget **400** (margins 2.97× / 2.53×), un-ducked **−14**. The plan arguments are pinned as measured: 150 / 200 / 400 / −120, **16** keyframes. *Was:* "gap = the final whole window of each 1 s gap … windows 1–4 are ramp or ducked, window 5 is un-ducked".
47. **A17 (P2-E2) — (e) renders at the DOCUMENT raster, and the profile raster becomes a hand-run lane.** Probe-2 measured the 8 s streaming lane at every shipped raster: **1920×1080 127.6 s**, **1080×1920 128.8 s**, **1080×1080 77.6 s**, **320×180 14.7 s** — so N2 Q8's fallback, "swap to the aspect profile with the smallest raster", does **not** rescue it at 3× its own 25 s trigger. And **every audio measurement is raster-invariant**: integrated, momentary, short-term and LRA identical to the last centi-unit at all four, true peak moving **1** centi-dBTP (AAC bit-allocation noise). Both (e) **media** lanes therefore keep **`settings.resolution = document.resolution`** (AU3's `lane_settings` override; the agent lane queues and cancels, item 56); the profile's declared raster is still asserted on the settings struct as one of B10's four difference-set fields; the raster-invariance table is recorded as **evidence**; and "delivery at the profile's own raster" moves to §13 and Appendix A as a **hand-run lane**. *Was:* `e_streaming` encoding 1920×1080, with a `[probe-2]` time and a conditional swap to the smallest aspect raster.
48. **A18 (P2-E3) — (d)'s failing direction is a master-track ripple, not an angle-track one.** `RippleDeleteClip` on an **angle** track leaves the master stem **bit-identical** (1 152 000 / 1 152 000): it shifts only clips whose **start is at or after** the ripple point (`crates/kinewright-core/src/operation.rs:216-228`), and the master clip starts at frame 0 and merely straddles it. **`sync_lock` governs which tracks participate, not which clips inside them move**, so draft v1's reasoning was wrong and its fixture could never have failed. The failing direction is **`SplitClip` on the master at 150 + `RippleDeleteClip` of the master tail**, which diverges at interleaved sample **576 000** = `150 × 1 920 × 2 channels` exactly, and it is the fixture N2 S19 already named, `au6_d_a_cut_master_track_is_not_continuous`. The angle-ripple case is kept as the **measured negative** — an angle ripple provably cannot disturb the master, which is the policy (d) exists to prove. *Was:* `au6_d_a_ripple_delete_shifts_the_master_stem`, asserted "not bit-identical".
49. **A19 — five constants and two report shapes are closed.** `AU6_PROFILE_BAND_MAX_DRIFT_TENTH_DB` is **not declared**: drift measured **0** over three fresh-engine runs, so the pin is an **exact `assert_eq!`** with margin "infinite (measured exactly zero)" *(was: a `[probe-2]` ceiling)*. `AU6_PROFILE_LEAKAGE_ALLOWANCE_TENTH_DB = **60**` (measured 27, 2.22×) on a **Hann main-lobe** bound, with the point-mass model named as the wrong model at a 450 allowance *(was: `[probe-2]`, model unstated)*. `AU6_HUM_DROP_MIN_DB_HUNDREDTHS = **2 400**` against a measured **4 970** (2.07×) — N2 Q4's **250** was probe-1's *tenth-dB* figure under a *hundredths* name and would have been a 19.9× margin *(was: 2 500, itself draft v1's restatement of that slip)*. `AU6_HUM_HARMONIC_DROP_MIN_DB_HUNDREDTHS = **500**` gated on the **first three** harmonics only, because `REPAIR_HUM_HARMONIC_COUNT = 3` leaves 240 Hz un-notched and its excess **rises 103** — a four-harmonic gate would fail on the canonical chain *(was: "each of the four harmonics")*. The learn range is pinned **`274..312`** exactly, since one frame moves nine bands by up to **29** tenth dB. The harmonic vector is **`[]` under one Goertzel block, not four `None`s**, so assertions use `.is_empty()`. And `TrackSilentInMix` tests **`integrated.is_none()`**, because a **video-only** track reports `audible: true, integrated: None` — `audible` is a mix-state property, not a "carries audio" property *(was: `!audible && integrated.is_none()`)*.
50. **Probe-3 (N3) — the de-click error drop is measured and its floor is 120.** `AU6_DECLICK_ERROR_DROP_MIN_TENTH_DB = 120` (**Floor**), measured **258** (258.8 on the instrument), margin **2.16×**, **9 fresh-engine runs, drift 0.000**. Measured with **AU5's definition** — the de-click node **alone** against the **click-free** programme, whole programme, both channels, `200·log10(rms(degraded − click_free) / rms(declicked − click_free))` — through the product path (`mix_audio_stems` on a Dialogue bus carrying only `audio_declick`), which is **bit-identical** to AU5's `process_buffer_static` route (`after_rms 0.000162046` on both). *Failing direction* **`au6_c_a_chain_without_the_declick_node_keeps_every_click`**: **12** clicks survive and the drop is **0.0**, because the bare stem is bit-identical to the authored buffer (`max_abs = 0`). The value **120** coincides numerically with two **non-budget** parameters in the same unit (`reduction_tenth_db: 120`, `|AU6_A_DUCK_DEPTH_TENTH_DB| = 120`); §2.8's distinctness rule covers **budgets**, so it stands, and 100 would collide with the two `tolerance_lu_hundredths = 100`. *Was:* `[probe-2]`, then `[probe-3]`, unmeasured by both earlier probes.
51. **A20 (P3-E1) — the click shape is pinned to the one every probe measured, and AU5's shape is named as the wrong model.** `AU6_CLICK_SAMPLES = 2` and `AU6_CLICK_AMPLITUDE_HUNDREDTHS = 50`: **+A then −A, both channels, written after noise and hum**; `AU6_CLICK_FRAMES = 8` and the never-valued `AU6_CLICK_AMPLITUDE` are **withdrawn**. Every (c) number in this contract — the four silence spans, `274..312`, the profile pin, SNR 2 041 → 3 183, the hum table, the speech loss 101 — was measured on the 2-sample click. AU5's 8-frame sign-alternating click at its ±0.9 makes the three clicked gaps read **−36.39 dBFS** as assets, **139** hundredths under −35.00, so `au6_c_every_authored_gap_is_below_the_silence_threshold` **fails** its 250 floor on three of four gaps; at ±0.5 it reads −38.65, **365** under, a **1.46×** margin below `FIXTURE_MINIMUM_MARGIN`. `au6_clicks_into` keeps its signature and documents the shape. *Was:* §3.2 item 6's 8-frame click with no amplitude.
52. **A21 (P3-E2) — the de-click gate cannot be measured on the canonical chain, so (c) gains a FOURTH document.** On the canonical chain the whole-programme drop reads **4.5** against the voice-only clean, **−116.9** against the click-free programme, and the node's own share is **0.5**; none admits a ≥ 2× floor and none is AU5's quantity. Core gains **`au6_c_declick_only_operations()`** — one `UpsertAudioBus` on the Dialogue bus carrying `audio_declick` **alone** at the planner's constants (`max_click_milliseconds: 1`, `detector_threshold_tenth_db: 240`, `lookahead_milliseconds: 3`; `crates/kinewright-agent/src/server.rs:10235-10241`); `au6_sources` gains **`pub fn au6_c_click_free_track() -> Vec<f32>`**; the new fixture is **`au6_c_the_declick_node_alone_drops_the_click_error`**; §4.1 row 13 stays `click_count 12 → 0` **on the canonical chain**; the node's in-chain contribution (**0.5** whole / **17.2** at ±10 ms / **71.6** at ±1 ms against clean) is manifest **evidence** labelled `declick.contribution_in_chain`, never a budget. A new non-vacuity rule asserts `au6_scenario_tracks(LocationDialogue)`'s A1 buffer **equals** `au6_c_click_free_track()` plus `au6_clicks_into`, sample-exact. The fourth document is a fixture document, not a fifth revision advance: §5.3's ledger is unchanged. *Was:* "on the canonical repair chain".
53. **A22 (P3-E3) — §2.6 item 4's hum payload reads `harmonic_count: 3`.** `REPAIR_HUM_HARMONIC_COUNT = 3` (`crates/kinewright-agent/src/server.rs:10228`), matching §2.4, §4(c)(2) and every probe. The **15** is the planner's **reported** `chain_lookahead_milliseconds` (`server.rs:9699`; 12 + 3, asserted at `:23560`), a response field, not a node parameter. *Was:* `harmonic_count: 5` and `chain_lookahead_milliseconds: 15` written as if they were payload.
54. **B1 (N4) — (a)'s and (e)'s picture is `au6_picture_source(frames)`, video-only, and `au6_muxed_source` sits in no document.** The picture asset was described three incompatible ways (video-only in §2.4 and §3.3; "the one audio-carrying muxed source is (a)'s, which (e) reuses" in §3.2 rule 7, §3.3 and §11.1), and no probe ever measured an (a) or (e) number with an audio-carrying asset on V1: both probes built (e) as "(a)'s canonical document plus a video-only BT.709 picture on V1" (probe-1 §5, probe-2 §3) and (a) with no picture at all. Since `crates/kinewright-media/src/export.rs` has zero `TrackKind` references, any PCM in a V1 asset would have joined the (a) mix and moved rows 1–4 and every (e) row. Rule: V1 carries `au6_picture_source(AU6_PROGRAMME_FRAMES)`, `MediaKind::Video`, in **both** (a) and (e) (`carries_picture: true` for (a), and (e) by identity); `Au6TrackRole::Picture` is added (S12); `au6_muxed_source` keeps only its E16 recipe role — rule 7 still asserts it muxes and probes `AudioVideo` — and appears in **no** document; §3.3's "only (a)/(e) need an audio-carrying source" and §11.1's "audio-carrying … for (e)'s encode" are deleted. *Was:* three descriptions, one of which put unmeasured PCM into the mix.
55. **B2 (N4) — the 8 s encode is a 200-frame document, and the truncation is pinned as probe-2 built it.** `ExportSettings` has no range (`crates/kinewright-core/src/media.rs:1015-1049`); an export renders the whole document. Probe-2's `interview_export_document(fixture, video, 200)` (`target/review/au6/probe/probe2-media-tests.rs:913-995`) kept (a)'s **300-frame assets**, put **200-frame clips** (`source_range 0..200`) on all four tracks with `document.duration = 200`, and **regenerated** the ducking curve from the turns that survive truncation — `0..50`, `75..125`, `150..200` — with its own `ducking_curve` helper (`:357-424`): the planner's sixteen keys were never clipped, applied or dropped. That helper's 200-frame curve is the ten keys `(0,−120) (55,−120) (65,0) (72,0) (75,−120) (130,−120) (140,0) (147,0) (150,−120) (199,−120)`: **ducked from the third turn's attack to the last frame, with no release**, because the third turn runs to the programme end. Rule: `Au6ScenarioSpec(Delivery).frames = AU6_ENCODE_PROGRAMME_FRAMES = 200` and `document_source: Some(Interview)` means "(a)'s assets and operations at (e)'s own `frames`"; (e)'s base document is (a)'s assets (300 frames, byte-identical buffers and the same picture) with every clip `0..200`; `au6_canonical_operations(Delivery)` returns (a)'s five operations with operation 5's curve replaced by **`AU6_E_DUCK_KEYFRAMES`** — the **ten** keys of `AU6_A_DUCK_KEYFRAMES` with `at < 200`, last `(151,−120)`, so the bed stays ducked to frame 199 exactly as probe-2's curve did; the two curves differ only inside the ramps, by at most one frame. Item 7 asserts "(e)'s operations == (a)'s except operation 5, whose curve is the truncation" and "(e)'s document == (a)'s document truncated" (every clip `0..200`, duration 200), never byte equality of two documents of different length. Probe-2's ten-key curve is recorded in Appendix A row C13 as what rows 21, 22 and S1 were measured on; the fixture re-measures at §12 step 4 under margin-and-print (25× / 2.04× / 25×). The **agent lane's `au6_a5a` exports (a)'s 12 s document** at `source_master` and its cost is restated from probe-1 §5's 12 s row (**21 005 + 1 419 ms**); the eval lane's `a5a`/`a5b` stay on `fixture_au6_interview()` at 12 s. *Was:* "(e) reuses (a)'s canonical document byte for byte" at 300 frames while §4(e), §2.8, §11.3, C8, C10 and C11 all assumed 8 s; C11 "a5a 16".
56. **B3 (N4, option ii) — the agent lane's streaming test queues and cancels; it does not encode at 1080p.** `queue_export` has no raster override: `crates/kinewright-agent/src/export_queue.rs:784-793` builds `work.profile.export_settings(&document, …)` and sets only `loudness_normalization`, and `QueueExportArgs` (`server.rs:11891-11924`) has no resolution field, so `queue_export{profile: "youtube_1080p"}` renders **1920×1080** — 127.6 s for an 8 s programme (probe-2 §3), ≈ 190 s projected for the 12 s document the script exports, against a 180 s deadline. Rule: the test is renamed **`au6_a5b_the_streaming_target_is_reachable_by_the_agent`**; it asserts `get_delivery_profiles` reports `youtube_1080p` with `loudness_target.integrated_lufs_hundredths = -1400` and `true_peak_ceiling_dbtp_hundredths = -100`; that `queue_export{profile: "youtube_1080p", normalize_loudness: true, expected_revision, output_path}` is **accepted** — `structured_content.job.id` returned, `job.profile == "youtube_1080p"`, `job.state ∈ {"queued", "running"}`, and `get_export_jobs` lists that `id` with that profile; then `cancel_export{job_id}` (`server.rs:1089`, `:3971`) and a poll to a terminal state asserted **`!= "failed"`** (`"cancelled"` normally; `"completed"` tolerated by the deadline rule). No `audio_report` / `audio_verification` assertion, no encode awaited. The streaming encode's objective proof is the **media** lane at the document raster (A17) and the model lane's `a5b` (hand-run). §0.2 item 47's "both (e) lanes" means both **media** lanes; §2.7's raster cell reads "declared, not rendered (media); rendered only in the hand-run eval lane"; C11 and §11.3's agent breakdown are re-summed from probe-1 §5 without a5b's encode (N14): a1 5 / a2 9 / a3 15 / a4 5 / **a5a 24** / **a5b 2** = **60**. *Was:* `au6_a5b_the_delivery_lands_on_the_streaming_target` awaiting a 1080p encode inside a 180 s deadline, "a5b 16", and a breakdown that summed to 66.
57. **S1–S18 and N1–N17 of the consistency review (N4), each with its fix.** **S1** item 8 compares the un-overridden `profile.export_settings(document, Eight, default())` for the four-field difference set and asserts `au6_export_settings(job, document).resolution == document.resolution` separately for both jobs; 43b reads the un-overridden struct *(was: one `resolution` field asked to be both)*. **S2** one duck-gap derivation — A16's parked 600–850 ms ⇒ window 4 — and item 3 asserts the window index and the 500 ms zero *(was: "`end+5 .. next_start−4` = 8 frames", "19 of 12.5", and item 3 asserting "8")*. **S3** (d) is six splits and **four** deletes, V1 keeping `[0,75)` and `[150,225)`, V2 keeping `[75,150)` and `[225,300)`, at §2.6(d), §5.2, §6.1 and §7.4 *(was: three deletes)*; the 225 cut is unprobed geometry, measured at step 4. **S4** the hum payload carries `depth_tenth_db: -300`; `notch_q_hundredths` (neutral 1 200) and declick's `lookahead_milliseconds` (min = max = neutral = 3) are at neutral and **absent** on the wire *(was: `fundamental_hertz` and `harmonic_count` only)*. **S5** (b)'s chain has a constant → descriptor-parameter table (`makeup_gain_tenth_db`, `audio_parametric_eq.high_pass_hertz`, …). **S6** one row reuses a constant (row 4) and five carry none (8, 13, 16, 18, 19) *(was: four and three)*. **S7** `AU6_SOURCE_BUDGETS: [Au6Budget; 3]` holds the level tolerance, the band separation and the target separation; `AU6_BUDGETS` stays 22; §12's "stays in `AU6_BUDGETS`" sentence is dropped *(was: three rows with no array)*. **S8** §11.3's asserted key list gains `canonical_documents`, `pins`, `transcriptions`, `parity`, `multicam`, `delivery`; the raster table lives at `delivery.raster_invariance` only *(was: also under `performance`)*. **S9** `limiter_passes` is read from `ExportAudioReport` (`crates/kinewright-core/src/media.rs:1096-1101`), the rest from `DeliveryAudioVerification` (`:1131-1140`). **S10** `EvalDefinition` literals are **20 + 6 = 26** *(was: "twelve and four", "sixteen")*. **S11** (c) is speaker A's carrier on all four turns; (b)'s Voice A is speaker A's carrier and its ride is speaker B's. **S12** `Au6TrackRole::Picture`. **S13** §13's R30 line reads "2-sample ±0.5 impulses" *(was: 8-frame)*. **S14** `AU6_EXPLICIT_TEST_NAMES: [&str; 1]` names the published-manifest test, in §11.4 and step 13. **S15** `576 000 = 150 × 1 920 × 2 channels` *(was: `150 × 1 920`)*. **S16** the §0.3 sentence "probe-2's seven values" is read as "confirmed", per §12 step 4 and Appendix A; §0.3 itself is edited by the orchestrator's merge, not here. **S17** §5.5 gains the four-planner prose table; the reachable **refusals** with valid arguments are three (`plan_audio_ducking` rule 125, `plan_room_tone_fill`'s no-asset refusal, `plan_dialogue_repair`'s already-repaired refusal); `plan_clip_fades` has none and its pinned string is its "nothing to propose" *success* prose; `plan_audio_normalization`'s −32602 is a JSON-RPC error, listed as such; the test is renamed `au6_the_four_planners_prose_is_pinned_by_exact_string` *(was: "four refusals", two provocations named)*. **S18** item 19 no longer names `AU6_PROFILE_BAND_MAX_DRIFT_TENTH_DB`. **N1** the (a)(1) shortfall is **788** *(was: 1 200, the bed's shift)*. **N2** `mix_levels` costs **8.9–33×** `mix_window_levels` *(was: 6×)*. **N3** note 3 lists rows 8, 13, **16**, 18, 19, 20. **N4** eval items renumbered **67–78** and inventory **79–80** *(was: 66–77 / 78–79, with 66 used twice)*. **N5** "`au6_a3` has four advances (§5.3)" *(was: "(a3) … (§5.2)")*. **N6** (c)'s bus is written `Bus(Dialogue repair)` everywhere, the planner's own default name (`server.rs:9449`), with the evidence label `bus:dialogue_repair`; (a)'s "Dialogue" is a different bus. **N7** `Au6BudgetKind`'s `Floor` / `Ceiling` carry the 2× ratio rule, unlike `Cc7BudgetKind`'s code-distance semantics. **N8** §2.1's dependency list is complete. **N9** six new assertion variants and one reused *(was: seven)*. **N10** items 23 and 24 point forward to 49 and 46. **N11** §2.7's raster cell says declared, not rendered. **N12** `window_project_frames` is a `plan_clip_fades` response field, not an argument. **N13** P edits `HumanQuestion.id`'s doc comment (`eval.rs:1071`), stated in §7.9 and step 9. **N14** the agent breakdown sums to 60 *(was: stated 60, summed 66)*. **N15** `track_gaps` returns `Option<Vec<Range<TimeCode>>>` (`crates/kinewright-core/src/model.rs:1252`): `Some(vec![])` passes, `None` fails. **N16** 890 is labelled probe-1's superseded population beside the gate's 1 187. **N17** the token loop is `mixer_ui.rs:4290-4295` *(was: `:4287-4292`)*.

**No OPEN note remains.**

### 0.3 Changes from implementation and review

*Reserved.* The implementation records each deliberate deviation from the text below here, as AU2 §0, AU3 §0, AU4 §0 and AU5 §0 do. **Erratum numbers are reserved per crate so parallel implementers cannot collide:**

| Range | Owner |
| --- | --- |
| **R1–R19** | `kinewright-core` — `au6_scenarios.rs`, `tests/au6_core.rs` |
| **R20–R39** | `kinewright-media` — `au6_sources.rs`, `au6_fixtures.rs`, `tests/fixtures/au6_manifest.json` |
| **R40–R59** | `kinewright-agent` — the six `au6_` tests in `tests/mcp_server.rs` |
| **R60–R79** | `kinewright-app` — the person-path tests, the `MixerChain::Track` arm, `edit_diff.rs`, and the two `pub(crate)` promotions of §0.2 item 18 |
| **R80–R99** | eval — `src/eval.rs`, `src/bin/kinewright-eval.rs`, `benchmarks/auto-edit/v7/` |

An erratum outside its range is a review finding, not an erratum. The figures regenerated as the code lands are the two `performance` lanes (§11.3) and the Windows lines; probe-2's seven values and probe-3's floor (Appendix A) are **confirmed** by §12 step 4, never re-derived, and a budget is never widened (S16).

**Core (J), `au6_scenarios.rs` / `tests/au6_core.rs`, 2026-09-10:**

- **R1.** §11.2 item 1's failing direction is false: at 30 fps a frame **is** 1 600 samples and a 200 ms window **is** six whole frames (9 600 / 1 600). `au6_scenario_geometry_is_the_contract_table` asserts that, and takes 24 fps (2 000 samples, 4.8 frames) and 30000/1001 (1 601.6 samples) as the failing directions; what 30 fps does break is the 150 ms attack, which is 4.5 frames there.
- **R2.** N4 S4's "`notch_q_hundredths` and declick `lookahead_milliseconds` are at neutral and **absent** on the wire" is wrong: `plan_dialogue_repair` writes both through `static_audio_effect` (`crates/kinewright-agent/src/server.rs:9571`, `:9590`), core's `upsert_audio_bus` stores the bus verbatim with no neutral stripping (`crates/kinewright-core/src/operation.rs:1796-1810`), and the agent's own `au5_plan_dialogue_repair_builds_the_measured_chain_at_the_head_of_the_bus` asserts the committed nodes carry `("notch_q_hundredths", 1_200)` and `("lookahead_milliseconds", 3)` (`server.rs:23643`, `:23655`). `au6_c_repair_operations()` and `au6_c_declick_only_operations()` therefore carry both, or §5.1(4)'s document equality fails against the planner's commit. The hum payload is `fundamental_hertz 60, harmonic_count 3, depth_tenth_db −300, notch_q_hundredths 1 200`; the declick payload `max_click_milliseconds 1, detector_threshold_tenth_db 240, lookahead_milliseconds 3`.
- **R3.** `AU6_C_LEARN_SOURCE_RANGE = TimeCode(275)..TimeCode(312)` — the **authored** click-free gap in source frames — not the detector's 274: the capture that produced E15's recorded **44-frame / 1 466 ms** room tone was 37 project frames (`37 × 1 920 / 1 600 = 44.4` → 44 source frames at 30 fps; 38 frames would give 45). S15's identity mapping holds either way (the surviving right-hand clip carries `source_range 175..312` at `timeline_start 175`); `274` is the **repair** learn range `plan_dialogue_repair` finds itself and stays `AU6_C_LEARN_PROJECT_RANGE`.
- **R4.** §2.8 / §11.2 item 9's "`!= 100` against both targets' tolerance" cannot hold for `AU6_DELIVERY_TRUE_PEAK_MARGIN_MIN_HUNDREDTHS = 100` (§4.1 row 22, dBTP hundredths). `au6_budgets_are_distinct_from_every_neighbouring_constant` asserts `!= 100` for every **LU**-hundredths budget and within-unit distinctness for all, and records the two cross-unit coincidences — the 100 dBTP margin against the 100 LU tolerance, and `AU6_MASTER_PASSTHROUGH_MAX_LU_HUNDREDTHS = 50` against AU3's `FIXTURE_AAC_OVERSHOOT_BUDGET_HUNDREDTHS = 50` dBTP — as CC7 recorded `FEATHER 4` against `CODEC_MAX 4`. Units are carried by a new `Au6Unit` enum on `AU6_THRESHOLD_CONSTANTS: [(&str, i64, Au6Unit); 21]`, the manifest's `thresholds` source.
- **R5.** The base documents are pinned, because `au6_b_fade_operations()` names clip ids and §2.1 forbids a literal elsewhere: `Au6ScenarioSpec` gains `clips: &'static [Au6ClipSpec]` (id, track, asset, project range; source = project on every base clip) and `asset_frames: u32` ((e)'s assets stay 300 frames under 200-frame clips, B2). (b)'s track 2 is four clips — `0..90 / 90..103 / 103..120 / 120..300`, ids 2–5 — so `AU6_B_GATED_CLIP_IDS = [ClipId(3), ClipId(4)]`; (d)'s six splits allocate `AU6_D_SPLIT_CLIP_IDS = 6..=11` and the four deletes are `AU6_D_DELETED_CLIP_IDS = [8, 6, 2, 10]`.
- **R6.** `au6_canonical_operations(LocationDialogue)` returns the two commits that need no captured asset — `au6_c_gap_operations(AU6_C_RIGHT_CLIP_ID)` then `au6_c_repair_operations()` — and **`au6_c_canonical_operations_with_room_tone(asset)`** is the full gap → fill → repair ledger (the Action's `AddAsset` sits between the first two and is not a commit), CC7's `cc7_lut_backed_canonical_operations` precedent: the captured room-tone record cannot be pinned in a module that reads no file.
- **R7.** Two chain shapes, stated: (b)'s c1m nodes are written in the **app's** shape — every descriptor row explicit at its neutral, the c1m values applied, ids `max + 1` within the chain (`insert_audio_effect`, `crates/kinewright-app/src/mixer_pane_ui.rs:1962-1996`), which is why `AU6_PODCAST_COMPRESSOR_RMS_WINDOW_MS = 10` is on the wire although it is the neutral — so the person path and `upsert_audio_bus` land one document; (c)'s repair nodes are in the **planner's** sparse shape (§2.6(c)(4), "the planner's single operation"). The (c) person path (`insert_audio_effect` × 3 + `noise_profile_operation`) therefore writes rows the planner does not (`bypass`, `floor_offset_tenth_db`, `smoothing_milliseconds`); §6.1 step 6's equality for (c) is implementer M's to reconcile and is recorded here, not fixed in core.
- **R8.** `AU6_C_PROFILE_MONOTONE_FROM_BAND = 13`, not §2.5's "above band 8": the pin falls from band 8 (−502) to band 9 (−601) and again over bands 11–13 (the 120 / 180 / 240 Hz partials), and is strictly increasing only from band 13 (≈ 400 Hz). `au6_every_budget_carries_the_declared_margin` asserts both facts on the pin; item 36's media claim should read "from band 13".
- **R9.** §2.4 / §3.2 rule 4's "the learn gap longest by 21 frames" is **22** on the pinned detector spans (`38 − 16`); 21 is the authored 37 against 16. The test asserts ≥ 21 and prints the exact difference.
- **R10.** §2.6(a) / §2.3's "the detector opens each window ≈ 1 frame **early**" is the wrong direction: the pinned `AU6_A_DUCK_DETECTED_WINDOWS` open at 1 / 77 / 151 / 227 against authored 0 / 75 / 150 / 225 — one to two frames **late** — and close four late. The test asserts `open ∈ [start, start + 2]` and `close == end + 4`.
- **R11.** `Au6TrackSpec` gains `carrier: Option<Au6Speaker>` (N4 S11: `Some(A)` on every Voice A track and on (c)'s dialogue, `Some(B)` on every Voice B track, `None` elsewhere) and `AU6_C_VOICE_CARRIER = Au6Speaker::A`; `au6_profile_export_settings(job, document)` is published beside `au6_export_settings` as the un-overridden struct item 8 reads (N4 S1); `AU6_TASK_IDS`, `au6_turns_within(scenario)` (the turns that survive (e)'s truncation) and `au6_e_canonical_operations()` are additive.
- **R12.** §2.3's "every authored boundary — … and (c)'s 160 / 175 / 312 — is a multiple of 5" is false for **312** (`312 = 62 × 5 + 2`): (c)'s whole-programme `mix_window_levels` vector has 62 whole windows and a two-frame tail, and the pinned learn range `274..312` is not grid-aligned at either end. Nothing gates on it — (c)'s only window-indexed reading, the speech-loss gate, slices the four turn ranges, all inside `0..300` — and `au6_scenario_geometry_is_the_contract_table` asserts `312 % 5 == 2` and grid alignment for every other boundary. Also recorded there: the hum partials halve in **amplitude** per step (`0.5^h`, AU5's `hum_fixture` shape and probe-2's `hum()`), stated as `AU6_C_HUM_PARTIAL_AMPLITUDE_RATIO_HUNDREDTHS = 50` rather than a "−6 dB" constant, because `AU6_C_ANALYTIC_MEAN_BAND_TENTH_DB` was evaluated on the halving and a literal −6.00 dB moves band 10 by one tenth dB; and the Hann-main-lobe and point-mass bounds agree only from band **13** up (the 240 / 300 Hz lobes leak into bands 11–12), not from band 11.
- **R13.** §2.1's "the module carries **no `f64`**" is read as "no **stored** `f64`": every published constant is an integer, but the §2.5(ii) bound §2.2 itself specifies (`au6_c_analytic_mean_band_tenth_db`) is `10·log10` arithmetic and is computed in `f64` inside `au6_c_band_bound_tenth_db`, returning an `i32` through `profile_band_tenth_db` verbatim — CC7's `cc7_encode_bt709` precedent. §10.1's two float exceptions (the generators' PCM and the seam) gain this third, arithmetic-only one; no AU6 API returns a float. (Review pass 1, N3.)

**Media (K), `tests/fixtures/au6_manifest.json`, 2026-09-11:**

- **R20.** The live media-lane wall on this Linux debug lane at `049eb90` is **437 s** (`cargo test -p kinewright-media --lib au6_ -- --test-threads=1`, 59 passed, cargo `finished in 437.01s`); re-measured at `cd9042b` after the review response it is **458 s** (`finished in 457.55s`, 59 passed), which is the figure the manifest now carries with 437 beside it. A1's **≈ 90 s** was a probe projection, not a live fixture-suite measurement. `AU6_MEDIA_LANE_BUDGET_SECONDS` stays **180** — recorded, never gated, never widened (S16). The per-group `breakdown` keys remain probe-1's projection; neither run emitted per-test times, so they are not re-derived. *Was:* `performance.media_lane.measured_seconds: 90`.
- **R21.** §11.3's prose names **twenty-six** top-level keys and then asserts "**twenty-four**". The two the count dropped are `required_fixtures` and `manifest_self_test`, which the same sentence requires; the count follows the list and `au6_manifest_declares_every_required_fixture_and_constant` asserts **26**. *Was:* 24 keys, with both blocks absent.
- **R22.** The manifest is generated from the code rather than transcribed beside it. `thresholds` carried **16** keys against `AU6_THRESHOLD_CONSTANTS`' 21 and **nine** values disagreed with the constants — seven of them looser, including `hum_drop_min_db_hundredths: 600` against the 2 400 item 49 closed, `delivery_deviation_max_lu_hundredths: 100` against 25, and `duck_depth_min_hundredths: 900` against 400. **No gate was ever affected** (`au6_scenarios.rs:2010-2089` matches §2.8 row for row and every fixture reads the constant), but the manifest was a false record of them and §11.0.3 was unimplemented. Every key is now derived from the constant's own name (`au6_threshold_key`) and asserted equal to its value, and each carries its `Au6Unit` under `thresholds.distinctness.units`, so a key can neither be missing nor hold a second number. `budgets` was one `note`; it is now **25** rows — `AU6_BUDGETS`' 22 plus `AU6_SOURCE_BUDGETS`' three — each carrying `{ constant, kind, budget, measured, margin_hundredths, margin, failing_direction, failing_direction_measured, reported_not_gated }`, all generated from the arrays and asserted, with the margin string derived from the integers so the two cannot diverge. §11.2's `budgets.<group>` keys live under `budgets.fixture_groups.<group>`, because the row table owns the top level. `export_jobs` named `ebu_r128`/`EbuR128` and `streaming`/`Streaming1080`, neither a `DeliveryProfile` variant; the rows are now `AU6_EXPORT_JOBS`' own `e_source_master`/`SourceMaster` and `e_streaming`/`Youtube1080p`, asserted field by field. `scorecard.person_agent_parity` is published and asserted against the count of `Au6PersonPath::Expressible` (§6.6), beside the `person_identical_document` / `person_equivalent_document` split R61 forces. `sources` gains a row per authored buffer (role, authored level, frames) asserted against `au6_spec`, and the **exact** §3.3 argument vectors asserted equal to `AU6_VIDEO_ONLY_RECIPE` and `AU6_MUX_RECIPE`; **no `sha256` is recorded**, and the manifest says why — no AU6 fixture byte is checked in (`docs/MEDIA-POLICY.md`), so a digest would pin a generator against nothing. `manifest_self_test` asserts the key counts and scans the manifest for six placeholder needles, which is §11.3's "no unresolved placeholder" as a check rather than a promise. *Was:* nine wrong thresholds, an empty `budgets`, two invented export-job ids, and `scorecard: { part_a: "complete" }` — a self-declared string no gate read.
- **R23.** The media lane costs more on CI than on this VM, and §14's projection is out by 7×. Linux CI run **34633080767** at `0b2290b` takes **667 s** for the `kinewright-media` lib binary (691 tests) and the whole Linux job moves from **8 m 58 s** on `main` to **19 m 27 s**, so AU6 adds ≈ **10.5 minutes** per Linux run against §14's ≈ 150 s for both lanes together. Both figures are recorded — `performance.media_lane.measured_seconds` 437 (local, `--test-threads=1`) beside `measured_seconds_ci` 667, and `performance.linux_ci_job_seconds` for the job — and both lane budgets stay where they are: **recorded, never gated, never widened** (S16). `ci.yml` has no `timeout-minutes`, so the failure mode is slow rather than red. §13 gains an item that owns the lane cost. *Was:* §14's "≈ 150 s", with no CI figure anywhere.

**Agent (L), `tests/mcp_server.rs`, 2026-09-11:**

- **R40.** The live agent-lane wall at `080243e` is **134 s** (`cargo test -p kinewright-agent --test mcp_server au6_ -- --test-threads=1`, 8 passed, 134.24 s); re-measured at `cd9042b` with R43's two document equalities added it is **131 s** (131.16 s, 8 passed). `AU6_AGENT_LANE_BUDGET_SECONDS` stays **120** — recorded, never gated, never widened (S16). The per-script breakdown remains probe-1's 60 s projection (N14). *Was:* measured ≈ 60 s against budget 120.
- **R41.** A15 / §5.5 list `room_tone_capture_too_long` as a capture code reachable with bad arguments on the (c) document. (c)'s dialogue asset is **312 frames / 12.5 s**; `ROOM_TONE_MAX_CAPTURE_MILLISECONDS = 60_000` is unreachable without leaving the asset, and `source_end_frame > duration` is `invalid_source_range` first. `au6_a3` therefore asserts the reachable past-end `invalid_source_range` plus `capture_refused` (rejected confirmation). The 60 s cap stays unit-tested at `crates/kinewright-agent/src/server.rs:24179`. *Was:* `room_tone_capture_too_long` asserted from a3's bad-argument set on (c).
- **R42.** §5.2's a2 table orders mix + buses, then `plan_audio_normalization` at both targets. After `UpsertAudioBus` on Voice A / Voice B, `normalization_context` (`crates/kinewright-agent/src/server.rs:11021`, `:11068`) refuses "track selection already intersects audio bus". a2 therefore plans both LUFS targets **before** the mix/bus commit; both calls remain evidence-only and uncommitted (`applied` is omitted / Null, asserted `!= true`). The schema refusal (`profile: source_master` → JSON-RPC `-32602` missing `track_ids`) is unchanged. *Was:* mix + buses commit, then plan at both targets.
- **R43.** `au6_a2` and `au6_a3` skipped §5.1(4). Both asserted `query_document(&core) == before` around their evidence-only calls — which is §5.1(1), not (4) — and neither compared the **committed** document to `au6_canonical_operations(scenario)` on the base document; only `au6_a1` and `au6_a4` did. Both now do. (c)'s equality needs the capture's own `AddAsset`, whose record is written from a file `au6_scenarios` cannot read (R6): the asset is lifted from the committed document and everything else asserted, through `au6_c_canonical_operations_with_room_tone(room_tone.id)`. **§5.1(4)'s one documented exemption — "(c)'s 31 profile rows" — is not used**: the profile `plan_dialogue_repair` learns on the agent lane equals `AU6_C_LEARNED_PROFILE_TENTH_DB` exactly, so `au6_a3` compares the whole document. *Was:* four of six scripts discharging (4).

**App (M), `mixer_ui.rs` / `mixer_pane_ui.rs` / `timeline_ui.rs` / `inspector_ui.rs` / `app.rs` / `edit_diff.rs`, 2026-09-11:**

- **R60.** §0.2 item 18 / §12.2 step 7(a)'s two promotions, recorded as the erratum they were always owed: `clip_audio_operation` (`crates/kinewright-app/src/inspector_ui.rs:4503`) and `noise_profile_operation` (`crates/kinewright-app/src/app.rs:2647`) become `pub(crate)` so (b)'s and (c)'s person-path tests can live in `mixer_ui.rs`'s `mod tests` as §11.2 places them. Both are visibility-only; neither adds a caller outside the tests, and `AU6_TEST_SOURCES` names both files.
- **R61.** §6.1 step 6's document equality for **(c)** cannot hold, and R7 left it to this implementer to "reconcile and record". The two chain shapes are incompatible by construction: the app's `insert_audio_effect` writes **every descriptor row** explicitly at its neutral (`mixer_pane_ui.rs:2056-2090`, AU2 §6.8, so the menu can never diverge from the descriptor table), while `plan_dialogue_repair` writes the planner's **sparse** shape (`static_audio_effect`, `server.rs:11194`). The person's repair nodes therefore carry rows — `bypass`, `floor_offset_tenth_db`, `smoothing_milliseconds` and their kin — that the planner's do not, and `assert_eq!(document, canonical)` is false for (c) and for (c) alone. What `au6_c_a_person_can_repair_the_dialogue_and_fill_the_gap` asserts instead is stronger than the prose it replaces: the two repair buses carry the **same nodes, in the same order, with the same ids and the same values for every row the planner writes**; every row the person writes and the planner omits is at that descriptor's own neutral, named one by one; the planner writes no row the person cannot; and upserting the planner's own bus over the person's makes the two documents **equal**, which proves the divergence is confined to those rows and nothing else in the document moved. The test also asserts the two documents are **not** equal, so the erratum cannot outlive the condition that created it. §6.6's sentence is restated accordingly.
- **R62.** Three more visibility-only promotions, and one gap they exposed. `MixerChain::effects_mut` and `MixerChain::effect_mut` (`mixer_pane_ui.rs:112`, `:124`) and `linked_delete_operations` (`timeline_ui.rs:2706`) become `pub(crate)`, because §11.2 places (b)'s, (c)'s and (d)'s tests in `mixer_ui.rs` while the `+ Effect` insert, the card's parameter write and the blade's delete all live in the other two modules. With them the three tests drive the app's own builders rather than replaying core's operation vectors: (b) builds chain c1m through `+ Effect` and one card control at a time, (c) blades, fills, inserts the three repair nodes and learns the profile, and (d) resolves **each** blade's clip from the document the previous blade left behind, which is what makes the late-to-early order a measured fact rather than a restated one.
  **The gap.** `+ Bus` names a bus after its track's caption — `track_caption_and_icon(kind, index).0`, i.e. `"A1"` (`mixer_ui.rs:863-871`) — and §6.5 records that the app has **no bus-rename control**. The canonical names (`"Dialogue"`, `"Music"`, `"Voice A"`, `"Voice B"`, `"Dialogue repair"`) are therefore the one value of the person path that no gesture types. `au6_person_bus_operations` drives the same `MixerChainEdits` fold the button drives and supplies the name, and **asserts that the caption `+ Bus` would have chosen is not the canonical name**, so the gap is a failing assertion away from being noticed rather than a silent lift. It is the bus-rename control §13 already carries at half a day; AU6 does not ship it (it is a third product change, outside the two the exception rule allows).

**Review finding 1 (outside AU6 erratum ranges) — AU3's non-normalize export identity, resolved.** `au3_an_export_that_does_not_normalize_is_byte_identical` (`crates/kinewright-media/tests/au3_fixtures.rs`) failed on this Linux / FFmpeg 8 VM on both `origin/main` (`888fe38`) and this branch, same assertion: the two un-normalized exports were not byte-identical (`assert_eq!(off, off_again)`, cargo `finished in 20.51s` on main). `src/export.rs`, `src/engine.rs` and `tests/au3_fixtures.rs` were identical to main and AU6 changed no AU3 product code. **It is fixed by PR #6 (`07dd22b`, `video_options.set("threads", "1")` on the delivery libx264 path), merged to `main` at `d182d45`** — separately, as a review finding rather than an AU6 erratum, because those files are not AU6's. **PR #6 was necessary and not sufficient, and the rest is now measured (AU3 §0 E57, E58).** `main` is merged into this branch, so the branch carries the thread pin — and the fixture still failed on Linux CI run **34650711913** at `4c80a0b` with it in place. Reproduced here: it fails about **one run in three**, but only when several of `tests/au3_fixtures.rs`'s tests encode concurrently in one process; twenty-plus isolated runs across separate processes all produced identical bytes. On a reproduced failure the difference is **entirely in the H.264 stream** (1 211 586 against 1 211 668 bytes, from about frame 13) while the **AAC stream is byte-identical at 332 005 bytes**; all 120 composited RGBA frames and all 120 YUV frames handed to the encoder hash identically across the two exports; the encoder reports `thread_count == 1` at runtime; and the same libx264 build driven by the ffmpeg CLI under full CPU load is byte-deterministic with and without `-threads 1`. The residue is the encoder's own in-process behaviour under concurrent instances, which the loudness step neither owns nor can gate. **AU3 E58** narrows B6's comparison to the delivered audio stream — the bytes the claim is actually about, copied out with `-map 0:a -c copy` — and records the video-side nondeterminism as an unowned limit of this build. The fixture's failure message also names lengths and the first differing offset now, because `assert_eq!` on two megabyte `Vec<u8>`s printed nothing a CI log kept. The deferral this note used to carry — "a workspace-clean closer, blocked by the pre-existing AU3 byte-identity failure" — is discharged.

**Review finding 2 (outside AU6 erratum ranges) — NEITHER CI job failed on a failing test, and Windows has never run an AU6 test.** `cargo test --workspace` on `windows-latest` at `0b2290b` runs exactly **one** test binary — `kinewright_agent`'s lib unittests — which fails **4 / 491** and aborts cargo before `kinewright-core`, the 59 AU6 media fixtures, the app tests or `tests/mcp_server.rs` run. The job nevertheless reported `conclusion=success`, because the `pwsh` block ran `setup-ffmpeg.ps1; cargo build; cargo test; cargo clippy` without checking `$LASTEXITCODE`, so the step's status was clippy's. Two consequences, both recorded rather than papered over:

1. **The step now fails on the first failing command** (`.github/workflows/ci.yml`, this branch), so Windows CI reports what it finds. It went **red immediately** — run **34644673292** at `cd9042b` — on the four pre-existing failures below, which is what it should have been doing all along.
2. **Every "Windows CI green" claim about AU6 made before that run is withdrawn.** §13 checklist item 5 and Appendix A row W plan to transcribe "the first green Windows CI run's printed `AU6_*` margin lines"; no such run has ever existed, and none could for as long as these four failures were in the tree, which also puts the AU2–AU5 "Windows CI green" statements in doubt. Scope's "both CI operating systems" was **unproven for AU6 on Windows** and stays so until a Windows run gets past `kinewright-agent`'s lib binary.

The four failures are pre-existing and unrelated to AU6 — identical on `main`'s run 34637198357 — and neither file is in AU6's set, so they are part of this review finding rather than an erratum. **All four are fixed on this branch**, because leaving the job red would leave the exit-code fix indistinguishable from a regression:

| Test | Cause | Fix |
| --- | --- | --- |
| `export_queue::tests::au3_a_normalizing_job_runs_the_profile_target_and_records_its_report` | `resolve_output_path` records the **canonical** parent (`export_queue.rs:1237-1243`), while `std::env::temp_dir()` reads `TEMP`, which a GitHub Windows runner sets to the 8.3 short form. The test compared `C:\Users\RUNNER~1\…` against the queue's `\\?\C:\Users\runneradmin\…` — one directory, two names | `test_directory` returns `path.canonicalize()`, so every path the module builds is the one the queue publishes. A no-op on Linux |
| `export_queue::tests::cc6_a_verified_export_publishes_its_decoded_comparison_on_the_record` | the same, on `verification_calls()[0].0` | the same helper |
| `export_queue::tests::a_source_swapped_mid_encode_fails_the_job_instead_of_completing_it` | the same, inside the quarantine message the failure has to name | the same helper |
| `color_status::tests::circular_median_agrees_with_brute_force_and_survives_two_million_samples` | a wall-clock assertion, `< 2 s`, which read **2.36 s** on the runner. The claim is a complexity one and a wall clock cannot tell a slow box from a quadratic scorer | the bound is now **relative to one `sort_unstable` of the same data** — the sort dominates the function and everything after it is linear. Measured ratio **1.18×** locally against a bound of 50×; a quadratic scorer at two million samples would be ≈ 10⁵× the sort |

**Review finding 3 (outside AU6 erratum ranges) — the LINUX job had the identical defect, and `main` is red behind it.** Fixing Windows made the Linux job's own silence visible: run **34644673292** at `cd9042b` failed `cc7_fixtures::cc7_every_scenario_verifies_at_ten_bits` and reported `conclusion=success`, and so did **`main`'s** run **34637198357** at `d182d45`. Every "Linux CI is clean" statement in this document, including the one review finding 1 used to make, was therefore only as good as the run it cited — run **34633080767** at `0b2290b` is genuinely clean (30 `test result` lines, no failure), but that was luck rather than a gate.

**The root cause is shared by both jobs and lives in `scripts/setup-ffmpeg.sh`.** It snapshotted the caller's shell options with `_kinewright_setup_shopts="$(set +o)"` and restored them on exit. `$(…)` is a command-substitution subshell and **bash clears `errexit` inside one**, so the snapshot always read `set +o errexit` and the restore switched `-e` **off** in the caller. GitHub runs a `run:` block as `bash -e`, the first line of the block sourced this script, and every later failure was ignored. The script now snapshots `$-`, which the current shell expands, and restores `errexit`, `nounset` and `pipefail` from it; `ci.yml`'s Linux step additionally carries a `|| exit 1` per command, so a future regression in the script cannot disable the gate again. This also means **anyone who followed `AGENTS.md` and ran `source scripts/setup-ffmpeg.sh`** lost `-e` in that shell.

**The failing test is a nondeterministic encode measurement, re-baselined once with both numbers recorded.** CC7 scenario **f**'s ten-bit lane declares `luma_mean_code_millionths: 281` and `rgb_mean_code_millionths: 77 446`. x264 core 165's ABR frame-thread pool is not run-to-run deterministic — the finding PR #6 (`07dd22b`) landed `threads=1` for — so before that pin the term read 281 on an idle box and 274 on a loaded one, which is why the local pre-merge run passed and both CI runs failed. With the pin the lane is deterministic and reads **274** and **77 447**, measured twice here. Both manifest rows and their `margin` fields are updated (`3558.7189 → 3649.635`, `12.9122 → 12.9121`); **no budget moved** — both are ceilings against 1 000 000 and 274 is further inside it than 281 — and `CC7_MEASURED_DELIVERY_TEN`'s worst-per-term summary (`[1, 0, 347, 385 514, 4 129]`) is untouched, because scenario f is the worst for neither term. The eight-bit lane did not move. This is CC7's manifest, not AU6's, so it is a review finding rather than an R20–R39 erratum.

**Review finding 5 (outside AU6 erratum ranges) — with the gate working, Windows reaches `kinewright-media` for the first time and seven pre-existing failures appear. Two of them need a policy Riel owns.** Run **34659339451** at `4b5f0a7`: **Linux is green** — the first genuinely gated green in this repository's history — and Windows now runs **681 media tests** where it previously ran none, failing **7**. None is AU6's, and none is new code; they are what the swallowed exit code has been hiding. Triaged:

| Test | Cause | Class |
| --- | --- | --- |
| `lut_store::tests::store_root_is_the_project_stem_plus_the_asset_suffix` | the fixture hard-codes a POSIX path, so Windows resolves `/home/riel/edits/…` to `D:\home\riel\edits\…` | test hygiene, fixable |
| `loudness::tests::the_true_peak_detector_note_names_the_au3_meter` | an `include_str!` source-text pin searched for a needle that spans a newline; the Windows checkout is CRLF (the same class as review finding 4) | test hygiene, fixable |
| `derived_cache::tests::content_hashes_do_not_trust_same_size_same_mtime_replacement` | `PermissionDenied` at `derived_cache.rs:463` — the test rewrites a file Windows still holds open | test hygiene, fixable |
| `export::tests::delivery_filter_converts_sixteen_bit_full_range_rgb_to_limited_yuv420p` | neutral 8-bit chroma comes back as `{128}` on Linux and `{128, 129}` on Windows: the two FFmpeg packages round swscale's full→limited conversion differently | **measurement, policy** |
| `cc7_fixtures::cc7_every_scenario_verifies_at_eight_bits` | CC7's recorded decoded-difference measurements do not hold on the Windows encoder | **measurement, policy** |
| `cc7_fixtures::cc7_every_scenario_verifies_at_ten_bits` | the same | **measurement, policy** |
| `cc7_fixtures::cc7_manifest_declares_every_required_fixture_and_constant` | the same rows, through the manifest's summary | **measurement, policy** |

**Why the last four are not this slice's to fix.** They are exactly the risk §14 names: Windows CI provisions **a different FFmpeg package** — `System233/ffmpeg-msvc-prebuilt ffmpeg-8.0.1-r3` against Linux's `mifi/ffmpeg-builds 8.0-1` — and no AU3, CC6, CC7 or AU6 number has ever been measured with it, so "the first Windows run **is** the measurement". Every available response is currently forbidden by a contract: AU3 §0 E55, CC6 §6.3 and AU6 §10.6 all say a per-OS figure earns **a per-OS note on the constant's doc comment, never a per-OS constant**, and §4.1 note 1 forbids inventing a tolerance to absorb it. A single manifest cannot hold two platforms' measured columns. The three real options — provision one FFmpeg build on both operating systems, publish a per-OS measured column and amend the three contracts that forbid it, or scope the encoded fixtures to Linux and say so — are a cross-slice decision, not an AU6 erratum. §13 carries it.

**Review finding 4 (outside AU6 erratum ranges) — the first Windows run to reach the eval binary found an LF/CRLF comparison.** With the four `kinewright-agent` failures fixed, `windows-latest` got past that binary for the first time and failed `published_v6_manifest_tracks_the_color_workflow_suite` (`crates/kinewright-agent/src/bin/kinewright-eval.rs:7260`). `benchmarks/auto-edit/v6/ground-truth.json` is checked in with LF, `core.autocrlf` is `true` by default on a Windows checkout, so `include_str!` reads it back with CRLF while `cc7_ground_truth_json()` always generates LF. The compare was therefore asserting how the reader's git is configured. It now normalizes the checked-in side's line endings; **the file is not re-baselined with CRLF**, because that would break every other platform, and the generator's LF output stays the thing a maintainer pastes in. This failure is pre-existing and was simply unreachable while the job aborted earlier.

No OPEN note remains.

---

## 1. In scope and out of scope

AU6 delivers:

- **`kinewright_core::au6_scenarios`** (§2) — a public, dependency-free module that is the single authority for the five scenarios: geometry, authored turn ranges, source levels, the committed detector ranges, the canonical operations and export jobs, and every named budget constant.
- **`kinewright_media::au6_sources`** (§3) — a public generator module under the existing `#[cfg(any(test, feature = "test-util"))]` gate beside `cc7_sources`, authoring every AU6 PCM buffer in Rust as exact `wav_f32` bytes through `GeneratedMedia::from_bytes`, plus the video-only angle and picture sources and the **one** audio-carrying muxed source.
- **Technical gates as ordinary `cargo test`** (§4) — five gate families (a)–(e), default lane, both CI operating systems, no model, no network, **no audio device**.
- **Six scripted agent end-to-end tests** (§5) driving the real MCP endpoint with no LLM.
- **Five person-path tests** (§6) at the operation-builder level, plus the two exception-rule product fixes with their own tests and one `docs/DESIGN.md` edit.
- **A seventh eval suite `audio-workflow-v7`, and the harness plumbing it needs** (§7) — including the **audio evidence block** measured inside the runner, six new assertion variants and one reused, `is_audio_assertion`, `EvalDeliverableSpec.normalize_to_profile_target`, and an exporter on the eval server.
- **A blind review package** (§8) — the audio arms of both hard-coded benchmark-id branches, `AUDIO_WORKFLOW_NOT_APPLICABLE`, six single-clause questions, and a leak test.
- **`au6_fixtures.rs` + `au6_manifest.json`** (§11) — the audio programme's **first** fixture manifest, with a two-lane `performance` block.
- **The documentation deliverables** (§12.1, §12.4).

AU6 does **not** deliver: any new MCP tool or capability (the served surface stays at **7**, `INSPECTOR_TOOL_NAMES` at **84**, `operation_tools()` at **54**); any new `Operation` variant or effect descriptor; any change to the **core** `AudioChain` enum; a `plan_track_mix`; a de-esser or plosive remover; audio-based sync detection, audio-follow, a master-audio track type, or an angle viewer; a per-word ASR confidence or any intelligibility metric beyond WER; a dialogue-versus-music balance tool; a loudness-matched A/B hold; a bus solo/mute, scrub audio, or audio QC pane; a `RemoveAudioBus` or bus-rename control; an `expected_revision` on any planner; a live `KinewrightApp` UI harness; a seventh `HumanRatingDimension`; a recorded cross-platform decoded delta; surround, MIDI, plug-ins, or spectral editing; and any change to an AU3 or AU5 budget constant.

---

## 2. The scenario authority — `kinewright_core::au6_scenarios`

### 2.1 Module boundary

`crates/kinewright-core/src/au6_scenarios.rs`, `pub mod au6_scenarios;` in `lib.rs`, re-exported from the crate root. It is **data and arithmetic only**: no `Document` mutation, no rendering, no filesystem, no clock, no RNG, no PCM. It depends on `crate::model`'s `AudioBus` / `TrackMix` / `SyncGroup` / `Track` / `Clip` types, `crate::operation::Operation`, `crate::automation::{AutomationCurve, Keyframe, KeyframeInterpolation}`, `crate::effect::NOISE_PROFILE_PARAMETER_NAMES`, `crate::delivery::{DeliveryProfile, LoudnessTarget, DeliveryEncodeDepth}`, `crate::media::{ExportSettings, ExportCancellation}`, `crate::effect::{Effect, NOISE_PROFILE_BAND_COUNT}`, `Document` (as a `&Document` argument to `au6_export_settings` only — never mutated, never built), and the id and time types the §2.2 signatures use (`TrackId`, `ClipId`, `AssetId`, `AudioBusId`, `EffectId`, `TrackKind`, `TimeCode`), and nothing else.

It exists because five scenarios × three execution paths × two crates that also read them is seven places a level, a turn range or a budget could drift. **A number that appears in this module must not be restated as a literal anywhere else**; §11.3's manifest rule and §11.2's inventory make that checkable.

The module carries **no `f64`**: every stored quantity is an integer in the unit named in its identifier (§10.1).

### 2.2 Public items

```rust
pub enum Au6Scenario { Interview, Podcast, LocationDialogue, Multicam, Delivery }
pub const AU6_SCENARIOS: [Au6Scenario; 5];

pub enum Au6Speaker { A, B }
pub enum Au6PersonPath { Expressible, NotApplicable { reason: &'static str } }
pub enum Au6TrackRole { Picture, VoiceA, VoiceB, MusicBed, Dialogue, Angle, Scratch, MasterAudio }

pub struct Au6Turn { pub speaker: Au6Speaker, pub start: TimeCode, pub end: TimeCode }
pub struct Au6TrackSpec {
    pub track: TrackId,
    pub kind: TrackKind,
    pub role: Au6TrackRole,
    pub level_dbfs_hundredths: Option<i32>,   // None for a picture-only angle track
    pub sync_lock: bool,                      // true on every (d) track (§0.2 item 21)
}
pub struct Au6BusSpec { pub bus: AudioBusId, pub name: &'static str, pub tracks: &'static [TrackId] }

pub struct Au6ScenarioSpec {
    pub scenario: Au6Scenario,
    pub id: &'static str,                       // "a".."e"
    pub title: &'static str,
    pub fps: u32,
    pub frames: u32,
    pub sample_rate: u32,
    pub channels: u16,
    /// `Some(other)` when this scenario reuses another's assets and operations
    /// at its own `frames` — only (e) does, and it reuses (a) at 200 frames (B2).
    pub document_source: Option<Au6Scenario>,
    /// S18: a picture asset exists in this scenario's document.
    pub carries_picture: bool,
    /// S18 + A1: this scenario runs an encode and a delivery verification.
    /// (a) false, (b) false, (c) false, **(d) false** (its delivery leg is cut,
    /// A1), (e) true.
    pub delivers: bool,
    pub tracks: &'static [Au6TrackSpec],
    pub buses: &'static [Au6BusSpec],
    pub turns: &'static [Au6Turn],              // empty for (d)
    pub person_path: Au6PersonPath,
}

pub const AU6_SCENARIO_SPECS: [Au6ScenarioSpec; 5];
pub const fn au6_spec(scenario: Au6Scenario) -> &'static Au6ScenarioSpec;

pub fn au6_canonical_operations(scenario: Au6Scenario) -> Vec<Operation>;
/// S17: the base document's next free clip id, because §2.1 forbids reading a
/// `Document`. The two ids core then allocates are pinned below.
pub fn au6_c_gap_operations(next_clip_id: ClipId) -> Vec<Operation>;
pub fn au6_c_fill_operations(asset: AssetId) -> Vec<Operation>;
pub fn au6_c_repair_operations() -> Vec<Operation>;
/// A21: the FOURTH (c) document, for §4(c)(3) only — `audio_declick` alone on
/// the Dialogue bus. A fixture document, never a commit in §5.3's ledger.
pub fn au6_c_declick_only_operations() -> Vec<Operation>;
pub fn au6_d_angle_cut_operations() -> Vec<Operation>;
pub fn au6_b_fade_operations() -> Vec<Operation>;

pub struct Au6ExportJob {
    pub id: &'static str,                       // "e_source_master", "e_streaming"
    pub profile: DeliveryProfile,
    pub target: LoudnessTarget,                 // == profile.loudness_target()
    pub normalize: bool,                        // always true
    pub depth: DeliveryEncodeDepth,             // always Eight
}
pub const AU6_EXPORT_JOBS: [Au6ExportJob; 2];
pub fn au6_export_settings(job: &Au6ExportJob, document: &Document) -> ExportSettings;

/// (c)'s learned profile, a REGRESSION PIN (§2.5), not an analytic expectation.
pub const AU6_C_LEARNED_PROFILE_TENTH_DB: [i32; NOISE_PROFILE_BAND_COUNT];
/// The one-sided analytic bound of §2.5(ii): the mean-power band level of the
/// authored bed, per band, before the leakage allowance.
pub fn au6_c_analytic_mean_band_tenth_db(band: usize) -> i32;

pub struct Au6Budget {
    pub term: &'static str,
    pub constant: &'static str,      // "" for the five rows that carry none (8, 13, 16, 18, 19)
    pub budget: i64,
    pub measured: i64,
    pub kind: Au6BudgetKind,
}
pub enum Au6BudgetKind { Floor, Ceiling, Exact, MeasuredZero, TwoSided, RecordedMargin }
/// S2: one row per §4.1 row, not one per constant — one row reuses a constant
/// (row 4) and five carry none.
pub const AU6_BUDGETS: [Au6Budget; 22];
/// N4 S7: the three budgets that gate the sources and the two deliveries rather
/// than the mix — the authored-level tolerance, the band separation and the
/// target separation. Same shape, same margin rule, outside `AU6_BUDGETS`.
pub const AU6_SOURCE_BUDGETS: [Au6Budget; 3];
/// B2: (e)'s ducking curve — the ten keys of `AU6_A_DUCK_KEYFRAMES` with
/// `at < AU6_ENCODE_PROGRAMME_FRAMES`. A derivation, not a pin.
pub const AU6_E_DUCK_KEYFRAMES: [Keyframe; 10];

pub const AU6_QUESTIONS: [(&'static str, &'static str); 6];
```

`au6_canonical_operations` returns the operations in the order a commit must apply them and is the **single** definition of "the canonical document". `Au6BudgetKind` mirrors `Cc7BudgetKind`'s variant list (`crates/kinewright-core/src/cc7_scenarios.rs:1983-2004`) without `RatioAtLeastTwo`, **and its `Floor` / `Ceiling` mean something different**: CC7's carry a code-distance headroom, AU6's carry the 2× ratio rule of §10.5 (`measured/budget ≥ 2` and `budget/measured ≥ 2`), so `au6_every_budget_carries_the_declared_margin` is not CC7's assertion with the names changed (N7).

### 2.3 Geometry, normative

```text
AU6_SOURCE_FPS            = 25         AU6_SOURCE_WIDTH = 320   AU6_SOURCE_HEIGHT = 180
AU6_SAMPLE_RATE           = 48_000     AU6_CHANNELS     = 2
AU6_PROGRAMME_FRAMES      = 300        (12 s at 25 fps)
AU6_C_PROGRAMME_FRAMES    = 312        (12.48 s — (c) only, §2.4)
AU6_ENCODE_PROGRAMME_SECONDS = 8       AU6_ENCODE_PROGRAMME_FRAMES = 200   (A1)
AU6_WINDOW_MILLISECONDS   = 200        AU6_HOP_MILLISECONDS = 200
AU6_FRAMES_PER_WINDOW     = 5          AU6_SAMPLES_PER_FRAME = 1_920
```

**Why 25 fps and 200 ms.** `mix_window_levels` anchors window 0 at `range.start` and steps by `hop_frames` (`crates/kinewright-media/src/export.rs:1958-2008`). At 25 fps a project frame is exactly **1 920** samples and a 200 ms window exactly **9 600** = 5 frames, so every authored boundary — 0, 50, 75, 125, 150, 200, 225, 275, 300, and (c)'s 160 / 175 / 312 — is a multiple of 5 and the whole-programme window vector can be sliced by **integer index**. The probe confirmed the alignment exactly (`PROBE_A_ALIGNMENT range=50..75 frames=25 window_ms=200 windows=5`, with `report.range` echoing the request verbatim). This is what makes A1's one-render-per-mix-point rule possible: **N range-scoped renders become one**. `MixWindowRequest.window_milliseconds` is `1..=1000` and `hop_milliseconds` is `1..=window_milliseconds` (`crates/kinewright-core/src/media.rs:1355-1365`), so 200 / 200 is legal.

**Why every measured `mix_levels` window is ≥ 400 ms.** `LOUDNESS_GATING_BLOCK_FRAMES = 19_200` (`crates/kinewright-media/src/loudness.rs:27`, re-exported `lib.rs:101`) is 400 ms = **10 project frames**. AU6's shortest measured window is a 25-frame gap (**2.5×**) and its longest a 50-frame turn (**5.0×**). One `mix_levels` call reports every track, every bus and the master over its one range (`MixLevelReport`, `crates/kinewright-core/src/media.rs:1228-1239`), which is why A1 makes it **one call per authored turn**.

**The authored turn pattern**, in project frames, for (a), (b) and (c) — **except that (c)'s eighth gap runs to 312, stated here once** (nit N12):

| # | Owner | Range | Frames | Seconds |
| ---: | --- | --- | ---: | ---: |
| 1 | speaker A | `0..50` | 50 | 2.0 |
| 2 | gap | `50..75` | 25 | 1.0 |
| 3 | speaker B | `75..125` | 50 | 2.0 |
| 4 | gap | `125..150` | 25 | 1.0 |
| 5 | speaker A | `150..200` | 50 | 2.0 |
| 6 | gap | `200..225` | 25 | 1.0 |
| 7 | speaker B | `225..275` | 50 | 2.0 |
| 8 | gap | `275..300`, **(c): `275..312`** | 25 / **37** | 1.0 / **1.48** |

**Why the gap is 1 s and not 500 ms (A2), stated as arithmetic.** `plan_audio_ducking`'s shipped defaults are attack **150** ms, hold **200** ms, release **400** ms (`crates/kinewright-agent/src/server.rs:9849-9859`) = 750 ms of ramp and hold. Inside a **1 s** gap the hold and release (600 ms) sit at the head and the next turn's attack (150 ms) at the tail, so the bed is fully parked from **600 ms to 850 ms** into the gap — and exactly **one** aligned 200 ms window, **window 4** (600–800 ms), lies wholly inside that interval (A16's derivation, the only one this contract uses). Inside a **500 ms** gap hold + release alone exceed the gap, so **no** window is parked and gate (2) would measure ≈ 0 depth. The 1 s gap is therefore **required**, not preferred.

Named measurement windows, each a `const`:

```text
AU6_WINDOW_A_FIRST_TURN = TimeCode(0)   .. TimeCode(50)
AU6_WINDOW_B_FIRST_TURN = TimeCode(75)  .. TimeCode(125)
AU6_WINDOW_A_SECOND_TURN = TimeCode(150).. TimeCode(200)
AU6_WINDOW_B_SECOND_TURN = TimeCode(225).. TimeCode(275)
AU6_WINDOW_PROGRAMME    = TimeCode(0)   .. TimeCode(AU6_PROGRAMME_FRAMES)
```

**The duck-depth populations (B9 / N2 Q5, corrected by A16), normative.** Speech = the whole 200 ms windows **from the second window of each turn to its end** (36 windows); gap = **the FOURTH whole 200 ms window of each 1 s gap**. The arithmetic, stated because N2 Q5's version was off by one: hold **200** + release **400** ms of ramp sit at the gap's **head**, and the *next* turn's attack **150** ms sits at its **tail**, so the fully-parked interval runs from **600 ms to 850 ms** into the gap and only the **600–800 ms window — window 4 — lies wholly inside it**. Window 5 contains the attack ramp and measures **1.80 dB under parked**. Probe-2 measured the gap population `[-1994, -2004, -2002, -2000]` at window 4 against `[-2090, -2180, -2083, -2000]` at window 5; the gate clears the budget either way (**1 187** against **1 011**), and the corrected rule is the one that means what it says. The tail gap `275..300` has no following turn, so its windows 4 and 5 both read −2 000. `au6_scenarios` **pins the plan arguments** as constants equal to today's defaults, so a default change is caught:

```text
AU6_A_DUCK_ATTACK_MS = 150   AU6_A_DUCK_HOLD_MS = 200   AU6_A_DUCK_RELEASE_MS = 400
AU6_A_DUCK_DEPTH_TENTH_DB = -120
```

### 2.4 Source levels, authored and measured

Every level is an **authored** RMS in dBFS hundredths held by a named constant with its derivation in its doc comment. The measured column is the probe's; `au6_every_authored_level_matches_its_analytic_derivation` gates the pair.

**(a) Two-person interview with a music bed.**

| Constant | Authored | Measured |
| --- | ---: | ---: |
| `AU6_A_VOICE_A_LEVEL_DBFS_HUNDREDTHS` | −2 600 | **−2 602** |
| `AU6_A_VOICE_B_LEVEL_DBFS_HUNDREDTHS` | −3 200 | **−3 200** |
| `AU6_A_BED_LEVEL_DBFS_HUNDREDTHS` | −2 000 | **−2 000** |

Speaker A is a 64-partial Schroeder-phase carrier in the **1 kHz** third-octave band `[891.4, 1122.5)` Hz under a 4 Hz raised-cosine syllabic envelope; speaker B the same construction in the **2 kHz** band `[1782.9, 2245.0)` Hz. `AU6_VOICE_A_BAND_INDEX = 17`, `AU6_VOICE_B_BAND_INDEX = 20` (`exact_band_center(index) = 1000 · 2^((index−17)/3)`, `crates/kinewright-media/src/spectrum.rs:225`), **3 bands apart**. The bed is a steady 6-partial chord (220 / 277.183 / 329.628 / 440 / 554.365 / 659.255 Hz, quadratic phase spread) continuous over the whole programme.

**A 20 ms raised-cosine fade at every turn edge is normative** (`AU6_TURN_EDGE_FADE_MS = 20`): without it a turn boundary is a step, and the probe measured (b)'s compressor **increasing** the spread on stepped material.

**The trims are measured, and the nominal model is named as wrong (A3/E3).** `AU6_INTERVIEW_A2_TRIM_TENTH_DB = **37**`. The nominal `+60` — the authored 6.00 dB dBFS difference — makes gate (3) **worse than no trim**: measured **235** centi-LU against **365** un-trimmed and **5** at `+37`. K-weighting lifts the 2 kHz band ≈ 3.1 dB and the 1 kHz band ≈ 0.8 dB, so the pair is only **3.65 LU** apart in LUFS. The doc comment carries that derivation and names `+60` as the wrong model.

**(b) Podcast with uneven voices.**

| Constant | Authored | Measured |
| --- | ---: | ---: |
| `AU6_B_VOICE_A_LEVEL_DBFS_HUNDREDTHS` (steady) | −3 000 | **−3 005** |
| `AU6_B_VOICE_B_LEVEL_DBFS_HUNDREDTHS` (ride peak) | −1 800 | **−1 800** |
| `AU6_PODCAST_AM_HERTZ` | 1 | — |
| `AU6_PODCAST_AM_DEPTH_TENTH_DB` | 180 | — |
| `AU6_PODCAST_A_TRIM_TENTH_DB` | **72** | — |
| `AU6_PODCAST_B_TRIM_TENTH_DB` | **−72** | — |

**Carriers (N4 S11):** (b)'s Voice A is speaker **A**'s band-17 carrier, steady; its ride is speaker **B**'s band-20 carrier. Speaker B rides in the **log domain at 1 Hz over 18 dB, trough at both ends** (A5/E5). At 4 Hz a 200 ms window integrates 0.8 of a cycle and the bypass spread collapses from **1 482** to **377** centi-dB; at 1 Hz the measurement is also stable across window size (1 482 at 200 ms against 1 689 at 100 ms), so the uniform 200 ms grid holds. The brief's word "syllabic" is inaccurate for (b) and this contract calls it a level ride.

**(b)'s gated clips (A6/E6).** `AU6_B_GATED_CLIP_RANGES = [90..103, 103..120]` on track 2, both edges on envelope maxima and **strictly inside a turn**; **no gated clip starts at frame 0**. `plan_clip_fades` skips a clip whose head **or** tail window is silent, so its first attempt on wholly-authored clips proposed nothing; on these it proposes **1 frame in / 1 frame out per clip** (arguments `fade_milliseconds = 20`, `threshold_dbfs_hundredths = -4_000`; `window_project_frames = 1` is a **response** field the planner reports, not an argument — `ClipFadesPlanArgs` is `{ tracks, threshold_dbfs_hundredths, fade_milliseconds }`, `crates/kinewright-agent/src/server.rs:11774-11777`).

**(c) Noisy location dialogue (A4/E4).**

| Constant | Authored | Measured |
| --- | ---: | ---: |
| `AU6_C_VOICE_LEVEL_DBFS_HUNDREDTHS` | −2 400 | **−2 390** |
| `AU6_C_NOISE_LEVEL_DBFS_HUNDREDTHS` | −4 200 | — |
| `AU6_C_HUM_LEVEL_DBFS_HUNDREDTHS` (60 Hz + 4 decaying harmonics, −6 dB each) | −4 500 | — |
| learn-gap RMS (noise + hum) | — | **−4 026** |
| other-gap RMS | — | **−3 978** |
| `AU6_LEARN_GAP_BELOW_SILENCE_MIN_HUNDREDTHS` (floor) | **250** | **526** (2.10×) |
| nominal SNR, **reported not authored** | — | 1 636 (percentile estimator reads 2 041) |

**(c)'s voice is speaker A's band-17 carrier on all four turns** (N4 S11; probe-2's `utterances(…, A_CENTER, …)`, `target/review/au6/probe/probe2-media-tests.rs:815-817`), so every (c) number — 2 041 → 3 183, the 31-band pin, −4 026 — is a speaker-A measurement. `AU6_C_HUM_FUNDAMENTAL_HERTZ = 60`; the **fixture authors five partials** — 60 / 120 / 180 / 240 / 300 Hz, −6 dB decay — while `plan_dialogue_repair` notches **three** (`REPAIR_HUM_HARMONIC_COUNT = 3`), which is why §4(c)(2) gates the first three harmonics only and records the fourth (A19). `AU6_C_CLICK_COUNT = 12` at frames **10, 20, 30, 40, 60, 90, 100, 140, 180, 190, 210, 250** — under speech and in the three clicked gaps, **never in the learn gap**. The click shape is **`AU6_CLICK_SAMPLES = 2`** samples at **`AU6_CLICK_AMPLITUDE_HUNDREDTHS = 50`** — +A then −A, both channels, written **after** noise and hum (A20); the three clicked gaps then read **−39.78 dBFS** as assets, **478** under the threshold.

**The SNR bracket is withdrawn.** The brief's `[SNR 10 dB]` is **infeasible**: a −24 dBFS voice at 10 dB SNR puts the gap floor at −34 dBFS, *above* the −35.00 dBFS threshold, and `detect_silences` would never find the learn range. The rule is the floor under the threshold; the SNR is whatever that produces.

**The learn range is committed output, not the authored range (A12/E12).** The detector's 10 ms RMS window closes early, so the authored `275..312` commits as **`AU6_C_LEARN_PROJECT_RANGE = TimeCode(274)..TimeCode(312)`** — `plan_dialogue_repair` independently reports `learn_range = { track: 1, start_frame: 274, end_frame: 312 }`, and the four detected spans are `[(60,76), (124,140), (210,226), (274,312)]`, the learn gap longest by 21 frames. The authority publishes the authored range beside it as `AU6_C_LEARN_AUTHORED_RANGE`.

**`capture_room_tone` takes SOURCE frames (S15).** The Action's arguments are `source_start_frame` / `source_end_frame` on `asset_id` and it decodes the chosen **source** range (`crates/kinewright-agent/src/server.rs:2103`, `:2155-2160`, `CaptureRoomToneArgs` `:11856-11873`). The authority publishes **`AU6_C_LEARN_SOURCE_RANGE`** with the surviving right-hand clip's `source_range` beside it; after the split at 175 that clip starts at source frame 175 with no offset, so the project→source mapping is the **identity** and the contract states it rather than leaving it to be inferred.

**(d) Event / multicam with a master audio track.**

```text
AU6_D_MASTER_LEVEL_DBFS_HUNDREDTHS   = -2_000
AU6_D_SCRATCH_NOISE_LEVEL_DBFS_HUNDREDTHS = -3_400
AU6_D_ANGLE_OFFSETS_FRAMES           = [0, 8]        authored, caller-supplied
AU6_D_CUT_FRAMES                     = [75, 150, 225]
```

Scratch = the master signal delayed by the authored offset, **halved**, plus LCG noise. The `SyncGroup` is base-document state: `SyncGroup { id: 1, name: "Interview", members: [{ asset: angle1, offset: TimeCode(0), angle_name: "Angle 1" }, { asset: angle2, offset: TimeCode(8), angle_name: "Angle 2" }] }` (`crates/kinewright-core/src/model.rs:474-479`). **Every (d) track carries `sync_lock: true`** (§0.2 item 21) — V1, V2, A1, A2, A3 — without which `RippleDeleteClip` would not move the master stem and the failing direction could not fail.

**(e)** authors no material: it reuses (a)'s assets — the three buffers **and the video-only picture** (a) already carries on V1 (B1) — with every clip truncated to **`AU6_ENCODE_PROGRAMME_FRAMES = 200`** (B2, §2.6(e)).

### 2.5 (c)'s learned profile — a regression pin plus a one-sided analytic bound

N1's analytic 31-band array is **withdrawn as a gate** (§0.2 item 19). `plan_dialogue_repair` stores 31 measured `profile_bandNN_tenth_db` values from a **20th percentile over ten 4 096-frame windows** (`NOISE_PROFILE_PERCENT = 20`, `crates/kinewright-media/src/spectrum.rs:52-54`; `third_octave_band_percentile` `:355-425`), through a `band_level_hundredths` integrator that takes bins from `floor(low/bin−0.5)` to `ceil(high/bin+0.5)` with fractional overlap weighting (`:443-471`) — so the eleven bands from 20–200 Hz collect ≈ **one whole bin**, not their nominal third-octave width — and then through `profile_band_tenth_db`, which **floors toward −∞** and **clamps into `-1200..=0`** (`crates/kinewright-media/src/export.rs:1815-1834`). The percentile offset below the mean is band-dependent and has no stated closed form. Two claims replace the array.

**(i) A regression pin, labelled as such.**

```rust
/// REGRESSION PIN, not an analytic expectation. These are the values
/// `mix_noise_profile` produced over AU6_C_LEARN_PROJECT_RANGE at
/// Bus(Dialogue repair) on the probe machine at 11a6098. The claim is "the learner
/// still does what it did when this was measured", never "this is what the
/// authored bed implies".
pub const AU6_C_LEARNED_PROFILE_TENTH_DB: [i32; NOISE_PROFILE_BAND_COUNT] = [
    -853, -846, -809, -588, -497, -457, -530, -615, -502, -601, -580,
    -619, -643, -659, -645, -626, -618, -604, -593, -576, -566, -559,
    -546, -538, -530, -517, -505, -496, -484, -476, -463,
];
```

asserted with an **exact `assert_eq!`** (A19). Probe-2 re-learned the profile **three times, each with a fresh `FfmpegMediaEngine` and a freshly synthesised fixture**, and measured `max_abs_delta_tenth_db = 0`: there is no drift to budget, so `AU6_PROFILE_BAND_MAX_DRIFT_TENTH_DB` is **not declared**, the margin is **"infinite (measured exactly zero)"**, and the doc comment records the three-run measurement. The worst band's delta is still **printed**, because Windows is CI's and the margin-and-print rule applies there too. `plan_dialogue_repair`'s own `profile_bands_tenth_db` is asserted equal to `mix_noise_profile`'s over the same range as a second, independent claim.

**The learn range must be pinned exactly, and one frame moves nine bands.** Learning over `275..312` instead of the committed `274..312` moves band 0 from **−853 to −882**, `max |Δ| = 29` tenth dB. `au6_c_a_one_frame_learn_offset_moves_the_profile` asserts it, so a future transcription of the *authored* gap start fails loudly rather than silently breaking the exact pin.

**(ii) One analytic one-sided bound that is derivable.** Every learned band must be **≤** `au6_c_analytic_mean_band_tenth_db(band)` + **`AU6_PROFILE_LEAKAGE_ALLOWANCE_TENTH_DB = 60`** (ceiling; worst measured excess **27** at band 5, margin **2.22×**), where the analytic term is the **mean-power** band level of the authored white noise plus the hum partials, computed with the same bin-count integrator and half-bin overlap the runtime uses (bin width `48 000 / 4 096 = 11.718 75` Hz, band edges `1000·2^((k−17)/3)·2^(∓1/6)`, then `profile_band_tenth_db` verbatim). Each hum partial is modelled as a **Hann main lobe four bins wide** (`MAIN_LOBE_BINS = 4.0`, `crates/kinewright-media/src/spectrum.rs:70`), i.e. spread over `±2·bin_width = ±23.44` Hz before the band integrator sees it. **The point-mass model — a partial's whole power in the one band containing its frequency — is named as the wrong model**: it denies that a windowed tone leaks, its worst excess is **225** tenth dB in a band *adjacent* to 60 Hz, and it would need an allowance of **450** tenth dB = 45 dB, making every hum-adjacent band vacuous. Under the main-lobe bound the noise-only bands sit **2–94 tenth dB under** it — the 20th percentile's own downward bias — and the one-sided claim holds for all 31. The inequality holds by construction because a 20th percentile sits below the mean. The closed-form percentile derivation — the thing that would turn this into a two-sided gate — is **§13 with a cost**.

**Shape claims, gated because they are what the authored material implies.** Bands 4–6 (≈ 50–80 Hz, the 60 Hz fundamental) sit **13–20 tenth dB above** the broadband trend; bands 1–3 (20–40 Hz) fall away **30–40 tenth dB**, the absence of anything below 50 Hz; and above band 8 the profile rises **monotonically**, the white-noise third-octave slope (analytically 1.0 dB per band, read as ≈ 0.4 because the reduction is a percentile). `au6_c_the_learned_profile_has_the_authored_shape` asserts the three.

**The app side asserts something different and says so.** `kinewright-app` is a **binary crate with no `[lib]`**, so no other crate can call it, and `noise_profile_operation(document, chain, effect, bands: &[i32; 31])` is *handed* its 31 values. The app test asserts only that `write_noise_profile` (`crates/kinewright-app/src/app.rs:2672`) writes **all 31 rows it was given** into the named `audio_denoise` node. There is no cross-path profile comparison and this contract does not pretend there is one.

### 2.6 Canonical operations per scenario

Effect names from `crates/kinewright-core/src/effect.rs`; `Operation` payloads from `crates/kinewright-core/src/operation.rs`. **The canonical document is built by applying these through the real `apply_batch`, one operation at a time, with `Document::validate()` after each** (A11), never by writing a `Document` literal.

**(a) Interview.** `V1 = TrackId(1)` picture — `au6_picture_source(AU6_PROGRAMME_FRAMES)`, **video-only**, `Au6TrackRole::Picture` (B1) — `A1 = TrackId(2)` Voice A, `A2 = TrackId(3)` Voice B, `A3 = TrackId(4)` Music bed. Buses `AudioBusId(1)` "Dialogue" `[A1, A2]`, `AudioBusId(2)` "Music" `[A3]`.

| # | Operation | Payload |
| ---: | --- | --- |
| 1 | `SetTrackMix` (`operation.rs:108-116`) | `{ track: A1, gain_tenth_db: 0, pan_percent: 0, mute: false, solo: false }` |
| 2 | `SetTrackMix` | `{ track: A2, gain_tenth_db: AU6_INTERVIEW_A2_TRIM_TENTH_DB (37), … }` |
| 3 | `UpsertAudioBus` (`:80-82`) | `AudioBus { id: 1, name: "Dialogue", tracks: [A1, A2], gain_tenth_db: 0, gain_curve: None, effects: [], ducking_sidechain_tracks: [] }` |
| 4 | `UpsertAudioBus` | `AudioBus { id: 2, name: "Music", tracks: [A3], … }` |
| 5 | `SetTrackAutomation` (`:120-131`) | `{ track: A3, parameter: "gain_tenth_db", curve: Some(AutomationCurve { keyframes: AU6_A_DUCK_KEYFRAMES }) }` |

**`AU6_A_DUCK_KEYFRAMES` is committed output, not authored geometry (A12).** The sixteen keys `plan_audio_ducking` commits at the pinned arguments are

```text
(0,0) (1,-120) (54,-120) (64,0) (73,0) (77,-120) (129,-120) (139,0)
(147,0) (151,-120) (204,-120) (214,0) (223,0) (227,-120) (279,-120) (289,0)
```

with `KeyframeInterpolation::Linear`. The detector opens each window ≈ 1 frame early and closes it ≈ 4 frames late, so the detected windows are `{1,54} {77,129} {151,204} {227,279}` against authored turns `0..50 / 75..125 / 150..200 / 225..275`. **The authority pins the committed keys and publishes the authored turns beside them**; §11.0.1 records this as a regression pin.

**(b) Podcast.** `A1 = TrackId(1)` Voice A, `A2 = TrackId(2)` Voice B. Buses `AudioBusId(1)` "Voice A" `[A1]` (no chain), `AudioBusId(2)` "Voice B" `[A2]` (the chain).

| # | Operation | Payload |
| ---: | --- | --- |
| 1 | `SetTrackMix` | `{ track: A1, gain_tenth_db: AU6_PODCAST_A_TRIM_TENTH_DB (72), … }` |
| 2 | `SetTrackMix` | `{ track: A2, gain_tenth_db: AU6_PODCAST_B_TRIM_TENTH_DB (−72), … }` |
| 3 | `UpsertAudioBus` | `AudioBus { id: 1, name: "Voice A", tracks: [A1], effects: [], … }` |
| 4 | `UpsertAudioBus` | `AudioBus { id: 2, name: "Voice B", tracks: [A2], effects: [eq, comp], … }` |
| 5–6 | `SetClipAudio` (`:343-352`) ×2 | `au6_b_fade_operations()`: `fade_in_frames: 1, fade_out_frames: 1` on each of `AU6_B_GATED_CLIP_RANGES`' clips |

Chain **c1m**, every value a named constant, each mapped onto the descriptor parameter it writes (N4 S5; names from `crates/kinewright-core/src/effect.rs:1892-1950` and `:2014`):

| Constant | Node | Parameter |
| --- | --- | --- |
| `AU6_PODCAST_HIGHPASS_HERTZ` | `audio_parametric_eq` | `high_pass_hertz` |
| `AU6_PODCAST_COMPRESSOR_THRESHOLD_TENTH_DB` | `audio_compressor` | `threshold_tenth_db` |
| `AU6_PODCAST_COMPRESSOR_RATIO_HUNDREDTHS` | `audio_compressor` | `ratio_hundredths` |
| `AU6_PODCAST_COMPRESSOR_ATTACK_MS` | `audio_compressor` | `attack_milliseconds` |
| `AU6_PODCAST_COMPRESSOR_RELEASE_MS` | `audio_compressor` | `release_milliseconds` |
| `AU6_PODCAST_COMPRESSOR_KNEE_TENTH_DB` | `audio_compressor` | `knee_tenth_db` |
| `AU6_PODCAST_COMPRESSOR_MAKEUP_TENTH_DB` | `audio_compressor` | **`makeup_gain_tenth_db`** (not `makeup_tenth_db`) |
| `AU6_PODCAST_COMPRESSOR_DETECTOR` | `audio_compressor` | `detector` |
| `AU6_PODCAST_COMPRESSOR_RMS_WINDOW_MS` | `audio_compressor` | `rms_window_milliseconds` |

```text
AU6_PODCAST_HIGHPASS_HERTZ              = 80
AU6_PODCAST_COMPRESSOR_THRESHOLD_TENTH_DB = -400
AU6_PODCAST_COMPRESSOR_RATIO_HUNDREDTHS   = 400
AU6_PODCAST_COMPRESSOR_ATTACK_MS          = 5
AU6_PODCAST_COMPRESSOR_RELEASE_MS         = 50
AU6_PODCAST_COMPRESSOR_KNEE_TENTH_DB      = 60
AU6_PODCAST_COMPRESSOR_MAKEUP_TENTH_DB    = 130
AU6_PODCAST_COMPRESSOR_DETECTOR           = 1        (1 = RMS, 0 = peak)
AU6_PODCAST_COMPRESSOR_RMS_WINDOW_MS      = 10
```

**The makeup gain is load-bearing and the contract says why**: at `makeup 0` the compressed bus drops **12.5 LU** below the untouched Voice A bus, the voice delta reads **1 250** against the 150 ceiling and the mix LRA reads **1 274** against the 300 ceiling — both gates fail. c1m is the most moderate candidate that clears every budget (+13 dB makeup, inside the descriptor's +24 dB maximum, unlike the alternative c4m's +22 dB).

**`plan_audio_normalization` is an evidence call in (b), not a canonical operation (A8/E8).** It is called at **both** targets with `{ track_ids, target_lufs_hundredths, maximum_sample_peak_dbfs_hundredths, tolerance_hundredths }` and its `predicted.integrated` asserted within `AU6_NORMALIZATION_PREDICTION_MAX_LU_HUNDREDTHS = 25` (measured **1** at both targets: −2 299 against −2 300, −1 399 against −1 400). It builds **one deterministic delivery bus** — document state, orthogonal to (e)'s job-state normalization — and **its `UpsertAudioBus` is not part of (b)'s canonical document.**

**(c) Location dialogue.** One dialogue track `A1 = TrackId(1)`; bus `AudioBusId(1)` **"Dialogue repair"** `[A1]` — the name is `plan_dialogue_repair`'s own default for a bus it creates (`crates/kinewright-agent/src/server.rs:9449`), and this contract writes the measurement point **`Bus(Dialogue repair)`**, meaning `MixSpectrumPoint::Bus(AudioBusId(1))` in (c), with the evidence label `bus:dialogue_repair`; (a)'s bus named "Dialogue" is a different scenario's bus (N4 N6). **Four revision advances, three of them commits** (B11 / N2 Q6):

1. **commit — `au6_c_gap_operations(next_clip_id)`.** `SplitClip { clip: 1, at: TimeCode(175) }` **then** `SplitClip { clip: 1, at: TimeCode(160) }` — **late-to-early** (A10/E10): splitting at 160 first makes the right half a *new* clip and the second split fails `SplitOutsideClip { clip: ClipId(1), at: TimeCode(175) }`. Then `DeleteClip { clip: AU6_C_MIDDLE_CLIP_ID }`, leaving the interior gap `160..175`. `AU6_C_RIGHT_CLIP_ID` and `AU6_C_MIDDLE_CLIP_ID` are pinned constants and `au6_core_allocates_the_pinned_gap_clip_ids` proves core's allocation rule produces them (S17).
2. **`capture_room_tone` — its own advance, not a commit.** The Action applies `Operation::AddAsset` itself through `apply_operation_analyzing` (`crates/kinewright-agent/src/server.rs:2320-2324`) at `args.expected_revision`. It takes `AU6_C_LEARN_SOURCE_RANGE` and confirms first.
3. **commit — `au6_c_fill_operations(asset)`.** The `AddClip` operations `plan_room_tone_fill` proposes: **one tile** for `160..175`, `maximum_tiles: 64`, `minimum_gap_frames: 1`, after which `document.track_gaps(TrackId(1))` is **`Some(vec![])`** — `track_gaps` returns `Option<Vec<Range<TimeCode>>>` (`crates/kinewright-core/src/model.rs:1252`), `None` meaning no such track, which is a failure — and the clips read `[(1,0), (3,160), (2,175)]`. The captured tone probes at **30 fps** and 44 asset frames / 1 466 ms; the fill planner maps it onto the 25 fps grid and the contract records that.
4. **commit — `au6_c_repair_operations()`.** One `UpsertAudioBus` whose `effects` are, in order, `audio_denoise` (31 profile rows, `reduction_tenth_db: 120`, `lookahead_milliseconds: 12`), `audio_hum_removal` (`fundamental_hertz: 60`, **`harmonic_count: 3`** — `REPAIR_HUM_HARMONIC_COUNT = 3`, `crates/kinewright-agent/src/server.rs:10228`, A22 — and **`depth_tenth_db: -300`**, `REPAIR_HUM_DEPTH_TENTH_DB`, `:10230`, present on the wire because its neutral is 0; `notch_q_hundredths: 1_200` is written by the planner (`:9571`) but is the descriptor's **neutral** (`effect.rs:2269-2273`) and therefore **absent** on the wire, N4 S4), `audio_declick` (`max_click_milliseconds: 1`, `detector_threshold_tenth_db: 240`; `lookahead_milliseconds: 3` is a real descriptor row with min = max = neutral = 3 and is likewise **absent** on the wire) — `plan_dialogue_repair`'s single operation. J verifies every neutral against `effect.rs` before writing the builder, because §5.1(4)'s document equality asserts the absent ones absent. The planner's response **reports** `chain_lookahead_milliseconds: 15` (`server.rs:9699`, 12 + 3); that is a response field, not a payload.

**A fourth (c) document exists for one gate and is not a commit (A21).** `au6_c_declick_only_operations()` returns one `UpsertAudioBus` on the same "Dialogue repair" bus carrying **`audio_declick` alone** at the three constants above. §4(c)(3) renders it through `mix_audio_stems` against the **click-free** programme, because AU5's error-drop instrument is defined on the node alone (`crates/kinewright-media/src/audio.rs:11409-11449`) and on the canonical chain the quantity reads **4.5 / −116.9 / 0.5** (§0.2 item 52). It is built by the media fixture from (c)'s base document; it is not part of the canonical document, not in the agent script, and §5.3's four advances are unchanged.

**(d) Multicam.** `V1 = TrackId(1)`, `V2 = TrackId(2)`, `A1 = TrackId(3)` scratch 1, `A2 = TrackId(4)` scratch 2, `A3 = TrackId(5)` master audio; **`sync_lock: true` on all five**.

| # | Operation | Payload |
| ---: | --- | --- |
| 1–6 | `SplitClip` | `AU6_D_CUT_FRAMES` on each angle clip, **late-to-early**: 225, 150, 75 on V1 then 225, 150, 75 on V2 |
| 7–10 | `DeleteClip` ×4 | three cuts make four segments per angle; the alternate halves are deleted so **V1 keeps `[0,75)` and `[150,225)`, V2 keeps `[75,150)` and `[225,300)`** and exactly one angle is visible at every frame (N4 S3; probe-2 measured two cuts and three deletes, so the 225 cut is unprobed geometry — the master-stem identity does not depend on the cut count, and K measures it at step 4 under margin-and-print) |
| 11 | `SetTrackMix` | `{ track: A1, gain_tenth_db: 0, pan_percent: 0, mute: true, solo: false }` |
| 12 | `SetTrackMix` | `{ track: A2, …, mute: true, … }` |

**No operation touches A3.** `plan_speaker_multicam` is not used: it refuses a non-video target track (`crates/kinewright-core/src/multicam.rs:188-189`), emits `Operation::ThreePointEdit` (`:427-439`) rather than the blade operations a person uses, and needs a diarized transcript no synthetic fixture can produce (§13).

**(e) Delivery (B2).** (e) is (a)'s document **truncated to `AU6_ENCODE_PROGRAMME_FRAMES = 200`** plus the two export jobs of §2.7, because `ExportSettings` has no range and an export renders the whole document. Its base document is (a)'s assets — the three 300-frame buffers and the video-only picture, byte-identical — with every clip `0..200` (`source_range 0..200`) and `duration = 200`. `au6_canonical_operations(Delivery)` returns `au6_canonical_operations(Interview)` with **operation 5's curve replaced by `AU6_E_DUCK_KEYFRAMES`**: the ten keys of `AU6_A_DUCK_KEYFRAMES` with `at < 200` —

```text
(0,0) (1,-120) (54,-120) (64,0) (73,0) (77,-120) (129,-120) (139,0) (147,0) (151,-120)
```

— which leaves the bed **ducked from the third turn's attack to frame 199 with no release**, because that turn runs to the truncated programme's end. This is exactly the shape probe-2 measured on: its `interview_export_document(fixture, video, 200)` (`target/review/au6/probe/probe2-media-tests.rs:913-995`) kept the 300-frame assets, cut 200-frame clips, and **regenerated** the curve from the surviving turns with its own helper (`:357-424`) rather than clipping the planner's keys; the helper's ten keys `(0,−120) (55,−120) (65,0) (72,0) (75,−120) (130,−120) (140,0) (147,0) (150,−120) (199,−120)` differ from `AU6_E_DUCK_KEYFRAMES` only inside the ramps, by at most one frame (Appendix A row C13). Item 7 asserts **operations equal except operation 5**, and **document equal to (a)'s truncated** — never byte equality of two documents of different length. The agent lane's `au6_a5a` exports (a)'s **12 s** document (§5.2); the eval lane's `a5a`/`a5b` are 12 s too (§7.2).

### 2.7 The canonical export jobs

A loudness target is **job state**: `ExportSettings.loudness_normalization` (`crates/kinewright-core/src/media.rs:1043`), and `DeliveryProfile::export_settings` hard-sets it to `None` (`crates/kinewright-core/src/delivery.rs:182-185`). AU6 publishes a canonical **(document, job, report)** triple. **(d)'s job is cut (A1); there are two.**

| Job id | Scenario | `profile` | `target` | Integrated | Tolerance | True peak | Raster |
| --- | --- | --- | --- | ---: | ---: | ---: | --- |
| `e_source_master` | (e) | `SourceMaster` | `EBU_R128_PROGRAMME_TARGET` | −2 300 | ±100 | −100 | 320×180 |
| `e_streaming` | (e) | `AU6_STREAMING_PROFILE = Youtube1080p` | `STREAMING_PLATFORM_TARGET` | −1 400 | ±100 | −100 | **1920×1080, declared, not rendered** (media); rendered only in the hand-run eval lane (B3) |

`EBU_R128_PROGRAMME_TARGET` `crates/kinewright-core/src/delivery.rs:225-230`; `STREAMING_PLATFORM_TARGET` `:236-241`; the mapping `loudness_target()` `:197-204`.

**Both media lanes render at the DOCUMENT raster, and the contract says why (A17 / P2-E2).** `DeliveryProfile::resolution(source)` returns `DeliveryAspect::Widescreen.resolution()` for `Youtube1080p` (`delivery.rs:136-141`, `:122-129`), so an un-overridden `e_streaming` would upscale 320×180 to 1920×1080. Probe-2 measured all four rasters at 8 s and **no aspect raster is affordable**: 1920×1080 **127.6 s**, 1080×1920 **128.8 s**, 1080×1080 **77.6 s**, 320×180 **14.7 s** — so Q8's "swap to the smallest aspect raster" does not rescue it either, at 3× its own 25 s trigger. And **every audio measurement is raster-invariant**: integrated, momentary, short-term and LRA are *identical to the last centi-unit* at all four rasters, and the true peak differs by **1 centi-dBTP** between 1920×1080 and 1080×1080, which is AAC bit-allocation noise. Encoding at the aspect raster buys **zero** additional audio evidence for **8.7×** the cost.

`au6_export_settings` therefore returns `profile.export_settings(document, DeliveryEncodeDepth::Eight, ExportCancellation::default())` with `loudness_normalization` set to `Some(job.target)` **and `resolution` overridden to `document.resolution`** — AU3's own `lane_settings` shape (`crates/kinewright-media/tests/au3_fixtures.rs:225-234`). **Two structs, two assertions (N4 S1):** the profile's declared raster is asserted on the **un-overridden** `profile.export_settings(document, Eight, default())`, where it is one of B10's four difference-set fields; and `au6_export_settings(job, document).resolution == document.resolution` is asserted separately for both jobs. One `resolution` field cannot be both, so item 8 reads the first struct and 43b the second. The *product* fact is pinned without paying to encode it, and the raster-invariance table is recorded as **evidence** in the manifest. **Delivery at the profile's own raster is a hand-run lane**, §13 and Appendix A.

**The two jobs' un-overridden settings are compared field by field with `cancellation` excluded (B10).** `ExportCancellation`'s `PartialEq` is `Arc::ptr_eq` (`crates/kinewright-core/src/media.rs:1402-1406`), so two independently built settings are **never** equal and a whole-struct comparison could not hold. The difference set is exactly **`resolution`, `video_bitrate`, `audio_bitrate`, `loudness_normalization`**; `delivery_color` is `delivery_color_for_depth(document, depth)` — the same document at the same depth — and is **identical**.

**Neither target has an LRA maximum** (`loudness_range_max_lu_hundredths: None` on both), so both `*_loudness_range_over_maximum` codes stay unreachable and AU6 does not assert them.

### 2.8 Budget constants

Every threshold AU6 gates on is a `SCREAMING_SNAKE` constant in this module with its unit in the name. **No AU6 gate uses a literal, and no AU6 constant is a float.** Values are the probe's, adopted by N1.5.

```text
Constant                                            Kind        Budget   Measured   Margin
AU6_INTERVIEW_DIALOGUE_OVER_BED_MIN_LU_HUNDREDTHS   Floor          400        812    2.03x
AU6_DUCK_DEPTH_MIN_HUNDREDTHS                       Floor          400      1 187    2.97x
AU6_VOICE_MATCH_MAX_LU_HUNDREDTHS                   Ceiling        150     5 / 50   30x / 3.0x
AU6_PODCAST_SPREAD_REDUCTION_MIN_HUNDREDTHS         Floor          450        978    2.17x
AU6_PODCAST_LRA_MAX_LU_HUNDREDTHS                   Ceiling        300         50    6.0x
AU6_REPAIR_SNR_GAIN_MIN_HUNDREDTHS                  Floor          500      1 142    2.28x
AU6_HUM_DROP_MIN_DB_HUNDREDTHS                      Floor        2 400      4 970    2.07x
AU6_HUM_HARMONIC_DROP_MIN_DB_HUNDREDTHS             Floor          500      1 096    2.19x
AU6_DECLICK_ERROR_DROP_MIN_TENTH_DB                 Floor          120        258    2.16x
AU6_REPAIR_SPEECH_LOSS_MAX_DB_HUNDREDTHS            Ceiling        300        101    2.97x
(profile band pin: exact assert_eq!)                 MeasuredZero     0          0    infinite
AU6_PROFILE_LEAKAGE_ALLOWANCE_TENTH_DB              Ceiling         60         27    2.22x
AU6_LEARN_GAP_BELOW_SILENCE_MIN_HUNDREDTHS          Floor          250        526    2.10x
AU6_MASTER_PASSTHROUGH_MAX_LU_HUNDREDTHS            MeasuredZero    50          0    infinite
AU6_DELIVERY_DEVIATION_MAX_LU_HUNDREDTHS            Ceiling         25          1    25x
AU6_DELIVERY_TRUE_PEAK_MARGIN_MIN_HUNDREDTHS        Floor          100        204    2.04x
AU6_TARGET_SEPARATION_TOLERANCE_LU_HUNDREDTHS       Ceiling         25          1    25x
AU6_NORMALIZATION_PREDICTION_MAX_LU_HUNDREDTHS      Ceiling         25          1    25x
AU6_AUTHORED_LEVEL_TOLERANCE_HUNDREDTHS             Ceiling         25         10    2.5x     (S3)
AU6_VOICE_BAND_SEPARATION_BANDS                     Floor            1          3    3.0x     (S3)
```

**The two unit restatements (B8 / N2 Q4, closed by A19), recorded rather than silent.** `AU6_HUM_DROP_MIN_DB_HUNDREDTHS` and `AU6_REPAIR_SPEECH_LOSS_MAX_DB_HUNDREDTHS` measure fields the product publishes in **hundredths** (`hum_60_excess_db_hundredths`, `crates/kinewright-core/src/audio_repair.rs:117`; `MixWindowLevelReport.windows`, `crates/kinewright-core/src/media.rs:1381-1385`), while N1.5 A7 and N2 Q4 carried the pairs `510 → 250` and `10 → 30`, which are **tenth dB**. Probe-2 re-measured the hum drop directly in hundredths at **4 970**, so the constant is **2 400** at 2.07× — *not* 250, which would be a **19.9×** margin, a budget no measurement approaches and what CC6 rule 11.0.5 forbids from the other side. The speech-loss pair restates as **300 against 101** (2.97×), the same physical budget.

Exact constants, derived rather than measured:

```text
AU6_SOURCE_FPS = 25             AU6_SOURCE_WIDTH = 320        AU6_SOURCE_HEIGHT = 180
AU6_SAMPLE_RATE = 48_000        AU6_CHANNELS = 2              AU6_SAMPLES_PER_FRAME = 1_920
AU6_PROGRAMME_FRAMES = 300      AU6_C_PROGRAMME_FRAMES = 312  AU6_FRAMES_PER_WINDOW = 5
AU6_WINDOW_MILLISECONDS = 200   AU6_HOP_MILLISECONDS = 200    AU6_TURN_EDGE_FADE_MS = 20
AU6_ENCODE_PROGRAMME_SECONDS = 8    AU6_ENCODE_PROGRAMME_FRAMES = 200
AU6_TARGET_SEPARATION_LU_HUNDREDTHS = 900     (-1 400 - (-2 300), exact)
AU6_SILENCE_THRESHOLD_RESTATED_DBFS_HUNDREDTHS = -3_500        (media's, restated with its owner)
AU6_LEARN_MINIMUM_PROJECT_FRAMES = 12         (ceil(22 528 x 25 / 48 000))
AU6_MEDIA_LANE_BUDGET_SECONDS = 180           measured ~90 s, 2.0x    (A1, recorded not gated)
AU6_AGENT_LANE_BUDGET_SECONDS = 120           measured ~60 s, 2.0x    (A1, B3: a5a at 12 s, a5b queues and cancels)
AU6_C_CLICK_COUNT = 12          AU6_A_DUCK_KEYFRAME_COUNT = 16    AU6_E_DUCK_KEYFRAME_COUNT = 10   (B2)
AU6_CLICK_SAMPLES = 2           AU6_CLICK_AMPLITUDE_HUNDREDTHS = 50   (A20; +A then -A, both channels)
```

**Restated constants carry their owner.** Core has no path dependency on `kinewright-media`, so `DEFAULT_SILENCE_THRESHOLD_DBFS_HUNDREDTHS` (`crates/kinewright-media/src/derived.rs:33`), `NOISE_PROFILE_MINIMUM_FRAMES` (`crates/kinewright-media/src/spectrum.rs:48-49`, `pub(crate)`) and `LOUDNESS_GATING_BLOCK_FRAMES` (`crates/kinewright-media/src/loudness.rs:27`) are restated with owner/file/line comments and **cross-checked from the media side** by `au6_restated_constants_agree_with_their_owners`.

**Distinctness, normative.** Every AU6 budget is asserted distinct **within its unit** from `FIXTURE_LOUDNESS_BUDGET_HUNDREDTHS`, `FIXTURE_AAC_OVERSHOOT_BUDGET_HUNDREDTHS`, `VERIFY_TONE_TRUE_PEAK_BUDGET_HUNDREDTHS` (`crates/kinewright-media/tests/au3_fixtures.rs`), every AU5 budget, and from both `LoudnessTarget.tolerance_lu_hundredths` values (`100`) — AU3 §0 E55's rule, that a budget equal to a target's own tolerance measures nothing.

### 2.9 What `au6_scenarios` is not

It is not a renderer, not a fixture and not a test helper: it holds no `Analysis`, spawns no `Core`, reads no file and authors no PCM. It does not re-implement `mix_levels`, `mix_window_levels`, `mix_noise_profile`, `audio_qc`, `audio_repair`, the loudness meter, the limiter, or any planner's arithmetic. **The three regression pins it does carry are labelled as pins** and never as derivations: `AU6_A_DUCK_KEYFRAMES`, `AU6_C_LEARN_PROJECT_RANGE`, and `AU6_C_LEARNED_PROFILE_TENTH_DB`.

---

## 3. The source generators — `kinewright_media::au6_sources`

`crates/kinewright-media/src/au6_sources.rs`, **public under the existing test-util gate** beside `cc7_sources` (`crates/kinewright-media/src/lib.rs:29-36`):

```rust
#[cfg(any(test, feature = "test-util"))]
pub mod au6_sources;
```

`test-util` is `test-util = []` in `crates/kinewright-media/Cargo.toml` and no default build enables it; `kinewright-agent` already does. The module returns `test_support::GeneratedMedia` and calls `test_support::run_ffmpeg`, which **panics** on a missing binary and on a nonzero exit (`crates/kinewright-media/src/test_support.rs:294`), so the module doc states the test-support boundary in `cc7_sources`' own words (`cc7_sources.rs:1-13`). It is the **one** generator: the media fixtures, the agent tests and the eval fixture builders all call it.

### 3.1 Public functions

```rust
pub fn au6_voice_pcm(speaker: Au6Speaker, level_dbfs_hundredths: i32, frames: u32) -> Vec<f32>;
pub fn au6_ride_pcm(level_dbfs_hundredths: i32, am_hertz: u32, depth_tenth_db: i32, frames: u32) -> Vec<f32>;
pub fn au6_chord_bed_pcm(level_dbfs_hundredths: i32, frames: u32) -> Vec<f32>;
pub fn au6_noise_pcm(level_dbfs_hundredths: i32, frames: u32) -> Vec<f32>;
pub fn au6_hum_pcm(level_dbfs_hundredths: i32, fundamental_hertz: f64, harmonics: usize, frames: u32) -> Vec<f32>;
pub fn au6_clicks_into(mono: &mut [f32], frames: &[u32]);   // A20: AU6_CLICK_SAMPLES at ±AU6_CLICK_AMPLITUDE_HUNDREDTHS, documented in its doc comment
pub fn au6_scratch_pcm(master: &[f32], offset_frames: i64, noise_level_dbfs_hundredths: i32) -> Vec<f32>;

pub fn au6_scenario_tracks(scenario: Au6Scenario) -> Vec<(Au6TrackRole, Vec<f32>)>;   // interleaved stereo
pub fn au6_source(scenario: Au6Scenario, role: Au6TrackRole) -> GeneratedMedia;       // .wav, audio only
pub fn au6_picture_source(frames: u32) -> GeneratedMedia;                            // .mkv, VIDEO-ONLY
pub fn au6_angle_source(angle: usize) -> GeneratedMedia;                             // .mkv, VIDEO-ONLY
pub fn au6_muxed_source() -> GeneratedMedia;                                         // .mkv, picture + PCM — the E16 recipe check ONLY; in no document (B1)
pub fn au6_scenario_sources(scenario: Au6Scenario) -> Vec<GeneratedMedia>;
/// A21: (c)'s A1 buffer BEFORE `au6_clicks_into` — voice + noise + hum, interleaved
/// stereo — so `au6_scenario_tracks(LocationDialogue)` == this + the clicks by construction.
pub fn au6_c_click_free_track() -> Vec<f32>;
pub fn au6_analytic_level_dbfs_hundredths(samples: &[f32]) -> i32;
/// A11/E11: an audio-only WAV probes at `Rational::default()` = 30 fps, so
/// every asset is re-stamped onto the project grid or `validate()` refuses
/// with `IncorrectDocumentDuration`.
pub fn au6_stamp_on_project_grid(asset: MediaAsset, fps: Rational, frames: TimeCode) -> MediaAsset;
```

### 3.2 The PCM recipe, normative

Every buffer is authored sample by sample in Rust and written as **exact `wav_f32` bytes** through `GeneratedMedia::from_bytes` (`crates/kinewright-media/src/test_support.rs:449-468`, `:67-72`) — 48 kHz, stereo, IEEE-float tag 3. **Not `lavfi`**: the provisioned FFmpeg's `sine` emits −18 dBFS and `amix` renormalises (`crates/kinewright-media/tests/au5_fixtures.rs:9-12`). Mono is authored first and interleaved by a local `to_stereo`, AU5's shape (`au5_fixtures.rs:201-208`).

1. **Voice** — `AU6_VOICE_PARTIALS = 64` sinusoids across the speaker's third-octave band with **Schroeder phases** `phi_p = pi p^2 / P`, the AU5 precedent (`crates/kinewright-media/tests/au5_fixtures.rs:451-494`) whose doc records why a linear phase ramp breaks per-window stationarity. Partial amplitude is solved analytically, `A = sqrt(2·10^(L/10) / (P·envelope_ms))`. Speaker A occupies band **17** (1 kHz), speaker B band **20** (2 kHz); **(c)'s single voice is speaker A's carrier on all four turns, and (b)'s Voice A is speaker A's** (N4 S11). The envelope is a **4 Hz raised-cosine burst train** (mean square exactly 3/8) gated by the authored turns; outside a turn the sample is exactly `0.0`, so a gap on a voice track is digital silence. **A 20 ms raised-cosine fade sits at every turn edge** (`AU6_TURN_EDGE_FADE_MS`).
2. **Level ride** — (b)'s speaker B only: the same carrier under a **1 Hz log-domain envelope over `AU6_PODCAST_AM_DEPTH_TENTH_DB = 180`**, starting and ending at the trough. Not "syllabic": at 4 Hz a 200 ms window averages the modulation away (§0.2 item 5).
3. **Music bed** — a steady 6-partial chord, continuous, no envelope. AU4's lesson: constant material makes a windowed delta attributable.
4. **Noise** — `test_support::pseudo_random_amplitude(frames, amplitude)` (`test_support.rs:417`), the seeded xorshift64 whose RMS is exactly `a/√3`. No `rand`, no clock, no OS entropy.
5. **Hum** — 60 Hz plus 4 decaying partials at −6 dB per step, AU5's `hum_fixture` shape at 60 Hz rather than 50, so AU6 exercises the branch AU5's mains fixture does not.
6. **Clicks** — `AU6_C_CLICK_COUNT = 12` clicks of **`AU6_CLICK_SAMPLES = 2`** samples at **`AU6_CLICK_AMPLITUDE_HUNDREDTHS = 50`**: `+0.5` then `−0.5`, both channels, written **after** noise and hum, at the frames §2.4 lists (A20). **No click in the learn gap.** The click-free buffer is kept as `au6_c_click_free_track()` and the clicks are the documented last step. **AU5's 8-frame sign-alternating click is the wrong model here**: at its ±0.9 the three clicked gaps read −36.39 dBFS, 139 hundredths *under* the −35.00 threshold, failing rule 3's 250 floor; at ±0.5 they clear it at only 1.46×. The 2-sample click's `d2` is `4 × 0.5 = 2.0` against a 20 ms reference of ≈ 7e-3 RMS at the −42 dBFS floor, an 18× clearance, and `click_count` reads 12 exactly on every probe run.
7. **Scratch** — the master delayed by the authored offset, **halved**, plus LCG noise at `AU6_D_SCRATCH_NOISE_LEVEL_DBFS_HUNDREDTHS`.
8. **Every asset is re-stamped onto the 25 fps project grid** by `au6_stamp_on_project_grid` (A11/E11).

**Non-vacuity rules.** Each is an `au6_` fixture and runs **before** the gates that consume it.

1. **`au6_every_authored_level_matches_its_analytic_derivation`** — every generated buffer's `au6_analytic_level_dbfs_hundredths` is within `AU6_AUTHORED_LEVEL_TOLERANCE_HUNDREDTHS = 25` (ceiling) of the authored level; worst measured **10** ((c)'s voice, −2 390 against −2 400), margin 2.5×. *Fails:* a buffer authored one constant off.
2. **`au6_the_two_voices_occupy_disjoint_bands`** — `mix_spectrum` at `Track(A1)` and `Track(A2)` on (a)'s base document: the two peak bands differ by ≥ `AU6_VOICE_BAND_SEPARATION_BANDS = 1` (floor); measured **3** (bands 17 and 20), margin 3.0×. *Fails:* both voices generated in one band.
3. **`au6_c_every_authored_gap_is_below_the_silence_threshold`** — **per authored voice ASSET, and scoped to (c)** (S14). (a)'s bed is continuous at ≈ −20 dBFS over the whole programme, so a mix-point or master reading in a gap is ≈ −20 and could never be below −35; and only an asset-domain reading is what `timeline_silences` consumes (`dialogue_repair_learn_span` walks `silence_status(asset)`, `crates/kinewright-agent/src/server.rs:9752-9758`). The claim is (c)'s: every authored gap's asset RMS sits at least `AU6_LEARN_GAP_BELOW_SILENCE_MIN_HUNDREDTHS = 250` below −3 500; measured **526**. *Fails:* **`au6_c_the_brief_levels_never_reach_the_detector`** — the brief's 10 dB-SNR bed measures ≈ −34 dBFS and is asserted **above** the threshold.
4. **`au6_the_learn_gap_is_the_longest_detected_silence`** — `timeline_silences` over (c) returns four spans `[(60,76), (124,140), (210,226), (274,312)]` and the longest is `AU6_C_LEARN_PROJECT_RANGE = 274..312`, longer than the next by **21 frames** and ≥ `AU6_LEARN_MINIMUM_PROJECT_FRAMES = 12`. *Fails:* **`au6_c_a_click_in_the_learn_gap_splits_it`**.
5. **`au6_the_scratch_track_is_not_the_master`** — the scratch buffer differs from the master at the authored offset and is not all-zero. *Fails:* a zero offset.
6. **`au6_wav_round_trip_is_sample_exact`** — one generated `.wav` per generator re-decoded and compared sample-exact. *Fails:* an AAC mux of the same samples.
7. **`au6_the_source_shapes_are_the_contract_table`** (B6, split from draft v1's single rule) — **(d)'s two angle `.mkv`s are VIDEO-ONLY** and probe as `MediaKind::Video`; **(a)'s picture — which (e) reuses — is video-only** and probes as `Video`; and `au6_muxed_source()`, the **one** audio-carrying source, muxes and probes as `AudioVideo` at 25/1 with the authored raster **and appears in no document** (B1). *Every document's V1 asset is asserted `MediaKind::Video`.* *Fails:* an angle source built with `-c:a` probes as `AudioVideo` and is asserted not to.
8. **`au6_c_the_degraded_track_is_the_click_free_track_plus_the_clicks`** (A21) — `au6_scenario_tracks(LocationDialogue)`'s A1 buffer **equals** `au6_c_click_free_track()` with `au6_clicks_into` applied, **sample-exact**, and the two differ at exactly `12 × AU6_CLICK_SAMPLES × 2` samples. *Fails:* a click-free buffer regenerated with a different noise seed.

**Why (d)'s angles must be video-only.** `crates/kinewright-media/src/export.rs` contains **zero** `TrackKind` references — the mix does not filter by track kind — so an angle clip whose asset carried scratch audio would contribute it to the mix regardless of the `mute` on A1/A2, and gate 4(d)(2) would read a mix that still carries the scratch. The scratch audio is separate `.wav` assets on A1/A2.

### 3.3 The mux recipe, normative

**No AU6 document carries an audio-carrying muxed source** (B1): the recipe exists so the programme's first audio-in-picture mux is proven once (E16) and is available to a later slice. The recipe is CC7's (`crates/kinewright-media/src/cc7_sources.rs:512-542`) with a second input and an audio codec appended; the probe ran it and all three PCM codecs muxed, probed as `AudioVideo` and exported through the real path (`pcm_s16le` 6 729 ms, `pcm_f32le` 6 836 ms, `pcm_s24le` 6 886 ms):

```text
-f rawvideo -pix_fmt yuv444p -s 320x180 -r 25 -i <temp.yuv>
-i <temp.wav>
-vf setparams=range=limited:color_primaries=bt709:color_trc=bt709:colorspace=bt709
-c:v ffv1 -level 3 -g 1 -pix_fmt yuv444p
-color_primaries bt709 -color_trc bt709 -colorspace bt709 -color_range tv
-c:a pcm_s16le -shortest
```

output `.mkv` through `GeneratedMedia::ffmpeg(label, &arguments, "mkv")`, under `run_ffmpeg`'s standing `-hide_banner -loglevel error -y` prefix (`test_support.rs:294-306`). **`pcm_s16le` is canonical** — it is what AU3's `lane_media` already writes into `.mov` — and `pcm_f32le` / `pcm_s24le` are recorded as working alternatives, the first being the one to pick if a later slice needs bit-exactness with the authored buffer. **Nothing in the recipe is platform-specific; Windows is CI's to confirm.**

**(d)'s angle sources and (a)/(e)'s picture use `cc7_source`'s recipe unchanged — no second input, no `-c:a`.**

**Audio-only export is accepted, not refused** (A14): a document with a single `TrackKind::Audio` track and no picture exports successfully through `export_document_reporting` and writes the file. (a)–(c) therefore do **not** need a picture to be exportable; (e) carries one because the brief wants the delivery gated the way a real deliverable is.

**Drop guards, normative.** Both temp files — the raw `.yuv` and the raw `.wav`/`.pcm` — are removed by a `Drop` guard in `RawFrames`' shape (`crates/kinewright-media/src/cc7_sources.rs:449-487`), because `run_ffmpeg` **panics** on a nonzero exit and a plain `remove_file` after the call is unreachable on exactly the path that leaks. At 8 s × 25 fps of yuv444p 320×180 the `.yuv` is ≈ 34.6 MB.

**One shared `FfmpegMediaEngine`** across the AU6 media lane: AU3's four separate constructions dominate its 48.9 s.

---

## 4. Technical gates

Ordinary `cargo test`, default lane, both CI operating systems, **no model, no network, no audio device**. No AU6 file may name `KINEWRIGHT_AUDIO_TEST`; §11.4's guard asserts it.

**The call pattern is normative (A1).** `mix_levels` costs **8 829 ms** over the whole 12 s document against **266–990 ms** for a whole-programme `mix_window_levels` render, because it meters every track, every bus and the master with 8× oversampled true peak while the other is a plain RMS over one stem. Therefore: **`mix_levels` once per authored turn** (one report carries every track and bus for that range); **everything RMS from one whole-programme `mix_window_levels` render per mix point**, sliced by integer window index; and **`audio_qc`, not `mix_levels`, for LRA** — both populate `loudness_range_lu_hundredths` and `audio_qc` meters only the master.

**One instrument per gate, never both** (N1 S8): `mix_levels` is BS.1770-gated over one named window ≥ 400 ms; `mix_window_levels` is short-window RMS on the aligned 200 ms grid and is explicitly **not** BS.1770 (`crates/kinewright-core/src/media.rs:1367-1370`).

**Crate attribution, once.** `server.rs`, `schema.rs`, `runtime.rs`, `eval.rs`, `audio_qc_tool.rs`, `audio_repair_tool.rs`, `color_scopes.rs` and `bin/kinewright-eval.rs` are **`crates/kinewright-agent/src/`**. `media.rs`, `model.rs`, `operation.rs`, `effect.rs`, `delivery.rs`, `audio_qc.rs`, `audio_repair.rs`, `automation.rs` and `multicam.rs` are **`crates/kinewright-core/src/`**. `audio.rs`, `export.rs`, `spectrum.rs`, `derived.rs`, `loudness.rs`, `engine.rs`, `room_tone_store.rs`, `test_support.rs`, `cc7_sources.rs` and `au5b_fixtures.rs` are **`crates/kinewright-media/src/`**; `au3_fixtures.rs` and `au5_fixtures.rs` are **`crates/kinewright-media/tests/`**. `mixer_ui.rs`, `mixer_pane_ui.rs`, `inspector_ui.rs`, `timeline_ui.rs`, `export_ui.rs`, `edit_diff.rs` and `app.rs` are **`crates/kinewright-app/src/`**.

**`mix_audio_stems` and `MixStems` are `pub(crate)`** (`crates/kinewright-media/src/export.rs:1099`, `:999`), so every stem gate lives in `src/au6_fixtures.rs`, not `tests/` (A9/E9). AU6 does **not** widen the public surface.

### 4(a) Two-person interview with a music bed

1. **Dialogue over bed.** *Measures:* `mix_levels` (`crates/kinewright-core/src/media.rs:2127-2131`) **once per authored turn**, reading `report.buses` for "Dialogue" and "Music" out of each `MixLevelReport`, energy-averaged over the four turns. *Passes:* `dialogue − music ≥ AU6_INTERVIEW_DIALOGUE_OVER_BED_MIN_LU_HUNDREDTHS = 400`; measured **812** (Dialogue −2 158, Music −2 970), margin **2.03×**. *Fails:* **`au6_a_the_unducked_document_does_not_clear_the_bed`** — the mixed but un-ducked document measures **−388**, the bed *above* the dialogue, **788** centi-LU short of the 400 floor (the bed's un-ducked → ducked shift is 1 200). **The failing direction is the un-ducked document, not the un-mixed one**: an un-mixed document has no buses, so this gate cannot be *measured* on it, only refused.
2. **Duck depth.** *Measures:* `mix_window_levels` (`media.rs:2199-2203`) at `MixSpectrumPoint::Bus(Music)` over `AU6_WINDOW_PROGRAMME`, **one render**, sliced by index. *Sampling:* §2.3's populations — speech = the 36 whole windows from the **second** window of each turn to its end; gap = the **fourth** whole window of each 1 s gap (A16). *Passes:* `min(gap) − max(speech) ≥ AU6_DUCK_DEPTH_MIN_HUNDREDTHS = 400`; measured **1 187** (max speech −3 191, min gap −2 004), margin **2.97×**. A `None` window inside a speech population is a **failure**, not a skip. *Fails:* **`au6_a_the_unducked_bed_has_no_depth`**, measured **−14** — 414 centi-dB the wrong side of the budget.
   **`Bus(Music)` is normative:** with the curve on the bus, `Track(A3)` reads a duck depth of **−1** because the bus fader is downstream of the track stem, while `Bus(Music)` reads the same depth for **all three** curve owners (**890** under probe-1's superseded population; the gate's own figure is **1 187** at window 4, A16).
3. **Voices matched.** *Measures:* `mix_levels` over each speaker's own turns, `report.tracks` for A1 and A2. *Passes:* `|A1 − A2| ≤ AU6_VOICE_MATCH_MAX_LU_HUNDREDTHS = 150`; measured **5** at `AU6_INTERVIEW_A2_TRIM_TENTH_DB = 37`, margin **30×**. *Fails:* **`au6_a_the_untrimmed_voices_are_not_matched`**, the un-mixed document at **365**. **The nominal `+60` trim is asserted as the wrong model** — it measures **235**, worse than no trim (§2.4).
4. **No clipping.** *Measures:* `audio_qc` (`media.rs:2158-2162`) over `AU6_WINDOW_PROGRAMME`. *Passes:* `technical_pass == true` and `clipping.left.clipped_runs == clipping.right.clipped_runs == 0` **exactly**; measured **0 / 0** on every variant, true peak −1 080. Margin **infinite (measured exactly zero)**. *Fails:* **`au6_a_a_hot_master_clips_and_says_so`**.
5. **Curve-owner equivalence, evidence not a budget (A13).** *Measures:* `mix_audio_stems` inside `src/au6_fixtures.rs`. *Records:* `SetTrackAutomation`, the Music bus's `gain_curve` and the bed clip's `audio_gain_curve` produce a **bit-identical master and bus stem** (`assert_eq!` on the `f32` vectors, `max |Δ| = 0.000e0`); only the *track* stem differs, and only for the bus route. Cost **1.9 s** for both stem tests.

### 4(b) Podcast with uneven voices

1. **Voices matched.** As 4(a)(3) at `Bus(Voice A)` / `Bus(Voice B)` against the same ceiling **150**; measured **50**, margin **3.0×**. *Fails:* **`au6_b_the_raw_trims_do_not_match_the_voices`**, un-trimmed **1 442**.
2. **Dynamics reduced.** *Measures:* `mix_window_levels` at `Bus(Voice B)` over `AU6_WINDOW_PROGRAMME`, whole windows inside B's turns; `spread = max − min`. *Passes:* `spread(bypassed) − spread(chain) ≥ AU6_PODCAST_SPREAD_REDUCTION_MIN_HUNDREDTHS = 450`; measured **978** (1 482 bypassed against 504 through c1m), margin **2.17×**. **`Bus`, not `Track`:** a track point is pre-bus-chain (`media.rs:1242-1253`; `stem_at_point`, `crates/kinewright-media/src/export.rs:1836-1855`). *Fails:* **`au6_b_the_bypassed_chain_leaves_the_spread_intact`**, reduction **0**.
3. **Programme loudness range.** *Measures:* **`audio_qc`** over `AU6_WINDOW_PROGRAMME`, `master.loudness_range_lu_hundredths` (A1: `audio_qc` is the cheaper of the two functions that populate it). *Passes:* `Some(lra)` with `lra ≤ AU6_PODCAST_LRA_MAX_LU_HUNDREDTHS = 300`; measured **50**, margin **6.0×**. A `None` is a **failure**, never a pass. *Fails:* **`au6_b_the_makeup_less_chain_exceeds_the_loudness_range`** at **1 274**, and the un-trimmed document at **1 434**. **Cuttable** (§12).
4. **No clipping.** As 4(a)(4); **0** runs on every candidate.
5. **The two planners answer.** *Measures:* the `plan_audio_normalization` and `plan_clip_fades` responses in §5's (b) script. *Passes:* `plan_audio_normalization` at **both** targets predicts within `AU6_NORMALIZATION_PREDICTION_MAX_LU_HUNDREDTHS = 25` — measured **1** at each — and publishes `lossy_codec_peak_headroom_hundredths = 200`, `processing_ceiling_dbfs_hundredths = -300`; `plan_clip_fades` proposes `fade_in_frames: 1, fade_out_frames: 1` on **each** of `AU6_B_GATED_CLIP_RANGES`' two clips. *Fails:* **`au6_b_a_clip_whose_head_window_is_silent_gets_no_fade`** — a clip authored across an envelope zero is skipped with `reason = "the track stem is silent over this clip's head or tail window"`, so the positive claim is not vacuous.

### 4(c) Noisy location dialogue

0. **The bed is under the detector, and the learn gap is the longest span.** §3.2 fixtures 3 and 4. This gate runs **first**: everything below depends on the learn span existing.
1. **SNR gain.** *Measures:* `audio_repair` (`media.rs:2214-2218`) at `Bus(Dialogue repair)` over the whole programme, `snr_db_hundredths` (`crates/kinewright-core/src/audio_repair.rs:108`). *Passes:* `snr(after) − snr(before) ≥ AU6_REPAIR_SNR_GAIN_MIN_HUNDREDTHS = 500`; measured **1 142** (planner chain 2 041 → 3 183), margin **2.28×**. *Fails:* **`au6_c_an_unlearned_profile_moves_no_snr`** — all 31 rows at `PROFILE_BAND_NEUTRAL_TENTH_DB = -1_200` (`crates/kinewright-core/src/effect.rs:1438`), gain **132**, failing by 3.8×.
   **The neutral-profile control fails two gates at once, and that is recorded as a strength:** with the denoiser an identity the floor stays 12 dB higher and the de-clicker's relative second-difference threshold never fires, so all **12** clicks survive as well.
2. **60 Hz drop, with per-harmonic selectivity.** *Measures:* the same report's `hum_60_excess_db_hundredths` (`audio_repair.rs:117`) and the four-entry `hum_60_harmonic_excess_db_hundredths` (`:125`). *Passes:* `excess(before) − excess(after) ≥ AU6_HUM_DROP_MIN_DB_HUNDREDTHS = 2_400`; measured **4 970** (9 163 → 4 193 on the planner chain), margin **2.07×**; **and each of the FIRST THREE harmonics** drops by ≥ `AU6_HUM_HARMONIC_DROP_MIN_DB_HUNDREDTHS = 500` — measured **1 096 / 1 706 / 2 272** at 60 / 120 / 180 Hz, worst margin **2.19×**.
   **The fourth harmonic is recorded, never gated, and the contract says why (A19).** `plan_dialogue_repair` notches **three** harmonics (`REPAIR_HUM_HARMONIC_COUNT = 3`), so the fixture's 240 Hz partial is un-notched and its excess **rises by 103** hundredths as the notch shoulders and the denoiser lower everything around it. `hum_60_harmonic_excess_db_hundredths` reports **four** entries unconditionally, so a gate written "each harmonic drops" would **fail on the canonical chain**. AU6 gates three, asserts the fourth is **not** claimed — itself evidence of selectivity — and §13 carries the mismatch.
   **The `None` rule, stated exactly (A19).** `HUM_GOERTZEL_BLOCK_FRAMES = 24_000` (500 ms). At (c)'s 599 040 sample frames there are **24 whole blocks** and nothing is `None`. Below one block the shape is `hum_60_excess_db_hundredths: None` and `hum_60_harmonic_excess_db_hundredths: []` — **an empty vector, not four `None`s** — so any assertion uses `.is_empty()` and never an index. A `None` total or an empty vector on the canonical range is a **failure**. *Fails:* **`au6_c_a_chain_without_the_hum_node_leaves_the_mains_alone`**, drop **1 tenth dB**.
   **No gate asserts the absence of `mains_hum_present` after repair, and no gate reads the 50 Hz term (A7/S7).** The 60 Hz cascade's sixth-octave shoulders reshape 50/100/150 Hz enough that `hum_50_excess_db_hundredths` rises from **0 to 829** (planner) or **1 072** (media chain) and a *second* `mains_hum_present` appears after repair. §13 records it as an AU5 limit with a cost.
3. **Clicks removed, with a printed drop — two rows, two documents (A21).** *Measures:* `click_count` (`audio_repair.rs:126`) **on the canonical chain**, and the AU5-shaped RMS-to-RMS **error drop** on the **fourth document** — `au6_c_declick_only_operations()`, `audio_declick` alone — rendered through `mix_audio_stems` (`crates/kinewright-media/src/export.rs:1099`), post-effects `Bus(Dialogue repair)` stem, against the **click-free** programme `au6_c_click_free_track()`, whole programme, both channels: `drop = 200·log10(rms(degraded − click_free) / rms(declicked − click_free))` — AU5's instrument verbatim (`crates/kinewright-media/src/audio.rs:11409-11449`, `DECLICK_ERROR_DROP_BUDGET_TENTH_DB: f64 = 300.0` at `:11004`). *Passes:* row 13 — `click_count(after) == 0` **exactly** with `click_count(before) == AU6_C_CLICK_COUNT = 12` on `au6_c_repair_operations()`; row 14 — the drop ≥ **`AU6_DECLICK_ERROR_DROP_MIN_TENTH_DB = 120`** (floor); measured **258** (258.8 on the instrument), margin **2.16×**, drift **0.000** over nine fresh-engine runs, and the declick-only stem asserted **bit-identical** to `process_buffer_static`'s output (`after_rms 0.000162046` on both routes). The line is printed in AU5's exact shape with probe-3's values:
   ```text
   AU6_DECLICK measured_drop_tenth_db=258.8 quantity=rms_to_rms before_rms=0.003189 after_rms=0.000162046 clicks=12 margin=2.16
   ```
   (`clicks` is AU5's before-count, `detect_clicks` on the degraded programme.) **Why not the canonical chain:** against the voice-only clean the whole-programme drop is **4.5** and the de-click node's own share **0.5** — the denoiser's alteration of the voice and its 12 dB residual noise dominate the error — and against the click-free programme the chain reads **−116.9**, because it removes noise the reference still holds; neither admits a 2× floor and neither is AU5's quantity. The in-chain contribution (**0.5** whole / **17.2** at ±10 ms / **71.6** at ±1 ms) is recorded in the manifest as `declick.contribution_in_chain`, evidence only. **The whole-programme figure is the neighbourhood figure:** the node is the identity outside the 12 spans of 4 sample frames it repairs, so ±1 ms, ±10 ms and ±40 ms all read 258.8 and no separate window is published. *Fails:* **`au6_c_a_chain_without_the_declick_node_keeps_every_click`** — **12** clicks survive on the chain without the node, and the bare document's drop is **0.0** with a stem bit-identical to the authored buffer (`max_abs = 0`).
4. **Speech survives.** *Measures:* `mix_window_levels` at `Bus(Dialogue repair)` over the four turn ranges. *Passes:* `mean(before) − mean(after) ≤ AU6_REPAIR_SPEECH_LOSS_MAX_DB_HUNDREDTHS = 300`; measured **101** (−2 418 → −2 519), margin **2.97×**. The intelligibility proxy in the absence of speech, and the contract calls it one. *Fails:* **`au6_c_an_over_reduced_profile_eats_the_dialogue`**.
5. **The room-tone fill closes the gap and is seamless.** *Measures:* `document.track_gaps(TrackId(1))` after the fill commit, and AU6's own `assert_au6_seam` helper across each join of `160..175`. *Passes:* `track_gaps(TrackId(1))` is **`Some(vec![])`** (`None` would mean the track is gone and fails), the clips read `[(1,0), (3,160), (2,175)]`, and the seam is **exactly zero** — a `MeasuredZero` claim, not a budgeted ceiling: AU5's `assert_seam` asserts `measured <= ROOM_TONE_SEAM_BUDGET` **and** `measured <= 0.0` (`crates/kinewright-media/src/au5b_fixtures.rs:243-258`), and AU6 writes its own helper rather than reuse one that hard-codes AU5's budget. *Fails:* **`au6_c_a_one_frame_slip_breaks_the_seam`**.
6. **The learned profile holds its pin, its bound and its shape.** §2.5. *Passes:* an **exact `assert_eq!`** against `AU6_C_LEARNED_PROFILE_TENTH_DB` (drift measured **0** over three fresh-engine runs), worst band still printed; every band ≤ the analytic Hann-main-lobe bound + `AU6_PROFILE_LEAKAGE_ALLOWANCE_TENTH_DB = 60` — worst excess **27** at band 5, margin **2.22×**, with the **point-mass model named as the wrong model** (it denies that a windowed tone leaks and would need an allowance of **450**, making every hum-adjacent band vacuous); the three shape claims of §2.5; and the planner's `profile_bands_tenth_db` asserted equal to `mix_noise_profile`'s. *Fails:* **`au6_c_a_profile_learned_over_speech_is_not_the_noise_profile`** — the same call over a *turn* range.
7. **The percentile estimator is not fooled by continuous speech, and the contract says why.** `audio_repair.rs:66-75` warns that on continuous speech the 10th-percentile "floor" is quiet speech. Here the measured floor is **−4 071** against an authored learn-gap RMS of **−4 026** — within **0.45 dB** — because 39 % of the programme is noise-only and the 4 Hz envelope passes through zero eight times per turn. The property depends on the **gap fraction**, not on the estimator, and is recorded as evidence.

### 4(d) Event / multicam with a master audio track

1. **The master stem is bit-identical across the cuts.** *Measures:* `export::mix_audio_stems` (`crates/kinewright-media/src/export.rs:1099-1103`) over `AU6_WINDOW_PROGRAMME` on the base and the committed document, `stems.tracks` for A3. *Passes:* `assert_eq!` on the two `Vec<f32>` — **bit-identical, not within a tolerance**; measured `master_stem_identical=true len_before=1152000 len_after=1152000 master_bus_identical=true`, and the scratch clip count moves 1 → 2 so the cuts really happened. *Fails:* **`au6_d_a_cut_master_track_is_not_continuous`** — `SplitClip` on the **master** at frame 150 followed by `RippleDeleteClip` of the master tail. The stems diverge at interleaved sample **576 000** = `150 × 1 920 × 2 channels` exactly (288 000 sample frames), an integer the fixture pins.
   **The angle-ripple case is evidence, not a gate, and A18 corrects draft v1 here.** `RippleDeleteClip` on an **angle** track leaves the master stem **bit-identical** (`master_stem_identical=true`, 1 152 000 / 1 152 000): `RippleDeleteClip` shifts only clips whose **start is at or after** the ripple point (`crates/kinewright-core/src/operation.rs:216-228`), and the master clip starts at frame 0 and merely straddles it. **`sync_lock` governs which tracks participate, not which clips inside them move**, so draft v1's `au6_d_a_ripple_delete_shifts_the_master_stem` **could never have failed** for the reason it gave. AU6 records the measured negative instead — *an angle ripple provably cannot disturb the master, which is the policy (d) exists to prove* — as `au6_d_an_angle_ripple_leaves_the_master_stem_alone`.
2. **The scratch tracks contribute nothing.** *Measures:* `mix_levels` over `AU6_WINDOW_PROGRAMME`, `report.tracks` for A1 and A2. *Passes:* both report `levels.integrated_lufs_hundredths == None` — **exact, no budget**: a muted track reports `None`, not a very small number. **`is_none()` is the predicate, not `!audible` (A19):** a **video-only** track reports `audible: true, integrated: None`, because `audible` is a mix-state property (not muted, not gated out by a solo) and **not** a "carries audio" property, so `!audible` would be wrong for V1/V2 and right only for A1/A2. The muted scratch tracks are additionally asserted `audible == false`, which is true of them specifically. *Fails:* **`au6_d_an_unmuted_scratch_track_reaches_the_mix`**, `audible=true`, **−2 965 / −2 968**.
3. **The mix is the master.** *Measures:* `mix_levels`, `master.integrated_lufs_hundredths`, against the master-alone document. *Passes:* `|Δ| ≤ AU6_MASTER_PASSTHROUGH_MAX_LU_HUNDREDTHS = 50`; measured **0** (`cut_muted` −2 670 against `master_only` −2 670), margin **infinite (measured exactly zero)**. *Fails:* **`au6_d_the_unmuted_scratch_moves_the_master`** at **600** centi-LU, **12×** the budget — a far wider failing direction than draft v1's 412 / 98, because the scratch is now its own full-level audio track rather than a muxed stream.
4. **The programme audio is one continuous clip.** *Passes:* the existing `ProgramAudioContinuous` claim on A3 — exactly one media clip, `audio_gain_tenth_db == 0`, both fades zero, `speed_percent == 100`, `effects.is_empty()`, `transition_in.is_none()` (`crates/kinewright-agent/src/eval.rs:6821`). *Fails:* **`au6_d_a_cut_master_track_is_not_continuous`**.
5. **The cuts sit at the authored frames.** *Passes:* integer equality with `AU6_D_CUT_FRAMES = [75, 150, 225]`, and exactly one angle visible at every frame — V1 over `[0,75)` and `[150,225)`, V2 over `[75,150)` and `[225,300)` after the four deletes (N4 S3). *Fails:* an off-by-one cut frame.

**(d)'s delivery leg is CUT (A1).** The probe measured it at **21.6 s** (encode 20 145 ms + verify 1 448 ms) for a 12 s two-video-track programme, and (e) already proves the delivery path at both targets on (a)'s document. §13 records what is lost: (d) never encodes, so no gate proves a *multicam* document survives the encoder.

### 4(e) Encoded delivery at two loudness targets

Both **media** lanes run on (e)'s **200-frame document** — `AU6_ENCODE_PROGRAMME_SECONDS = 8` (A1), the truncation of B2 — **and at the document raster, 320×180** (A17). All four BS.1770 quantities stay populated at 8 s, LRA reads 5 centi-LU at both lengths, and the deviations are equal or better than at 12 s. **The agent lane is different and says so (B2, B3):** `au6_a5a` exports (a)'s **12 s** document at `source_master` (probe-1 §5's 12 s row: encode **21 005 ms** + verify **1 419 ms**), and `au6_a5b` queues the streaming job and cancels it without encoding; the eval lane's `a5a`/`a5b` are 12 s and hand-run.

0. **Raster-invariance, recorded as evidence, never gated.** Probe-2 measured (e)'s streaming lane at all four shipped rasters:

| Profile | raster | integrated | deviation | true peak | LRA | encode ms | verify ms | total |
| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| `source_master` | 320×180 | −2 300 | **0** | −1 204 | 5 | 13 667 | 1 020 | **14.7 s** |
| `youtube_1080p` | 1920×1080 | −1 400 | **0** | −304 | 5 | 126 387 | 1 225 | **127.6 s** |
| `square_social` | 1080×1080 | −1 400 | **0** | −303 | 5 | 76 323 | 1 279 | **77.6 s** |
| `vertical_short` | 1080×1920 | −1 400 | **0** | −303 | 5 | 127 538 | 1 301 | **128.8 s** |

   Integrated, momentary, short-term and LRA are **identical to the last centi-unit** at every raster; the true peak moves **1 centi-dBTP**, AAC bit-allocation noise. The aspect raster buys **zero** audio evidence for **8.7×** the wall clock, so both AU6 lanes override `settings.resolution = document.resolution` (§2.7) and the manifest records this table. **Delivery at the profile's own raster is a hand-run lane** (§13, Appendix A).

1. **Both encodes land on target.** *Measures:* two reports per lane (N4 S9) — the export's own `ExportAudioReport` (`crates/kinewright-core/src/media.rs:1096-1101`, `report.audio` from `export_document_reporting`) for **`limiter_passes`**, and `Analysis::verify_delivery_audio` (`media.rs:2041-2045`) on the written file, a `DeliveryAudioVerification` (`:1131-1140`), for the rest. *Passes:* `technical_pass == true`, `exceptions == []` (verification), `limiter_passes == 1` (export report), and `|measured − target| ≤ AU6_DELIVERY_DEVIATION_MAX_LU_HUNDREDTHS = 25`; measured **0** on both 8 s lanes (worst across all lanes **1**), margin **25×**. **The budget is chosen against the measurement, never against the profile's own ±100 tolerance** (AU3 §0 E55). *Fails:* **`au6_e_an_unnormalized_export_misses_the_target`** — the identical document at `loudness_normalization: None` misses by **122** (source master) and **778** (streaming), i.e. 5–31× the budget.
2. **True peak clears the ceiling.** *Passes:* `ceiling − measured ≥ AU6_DELIVERY_TRUE_PEAK_MARGIN_MIN_HUNDREDTHS = 100`, printed; thinnest measurement is the **streaming** lane's **204** centi-dBTP, margin **2.04×** (source master keeps 1 104). *Fails:* the same un-normalized export.
3. **The two deliveries separate by the target difference.** *Passes:* `|(streaming − source_master) − AU6_TARGET_SEPARATION_LU_HUNDREDTHS (900)| ≤ AU6_TARGET_SEPARATION_TOLERANCE_LU_HUNDREDTHS = 25`; measured **900** at 8 s (**901** at 12 s), so the term is **0–1**, margin **25×**. **Cuttable** (§12). *Fails:* **`au6_e_two_exports_at_one_profile_do_not_separate`**, measured **0** separation.
4. **The job is the only thing that differs.** *Passes:* a **field-by-field** comparison of the two **un-overridden** `profile.export_settings(document, Eight, default())` structs **excluding `cancellation`** (B10), whose difference set is exactly `resolution`, `video_bitrate`, `audio_bitrate`, `loudness_normalization`; separately, `au6_export_settings(job, document).resolution == document.resolution` for both jobs (S1); and the **document is byte-identical** between the two exports. *Fails:* a document edited between them.

### 4.1 Budgets — `budget | measured | margin`

Linux, debug lane (what CI runs), one shared `FfmpegMediaEngine`, encodes at 8 s. `au6_every_budget_carries_the_declared_margin` asserts each row by its `Au6BudgetKind`: `measured/budget ≥ 2` for a **Floor**, `budget/measured ≥ 2` for a **Ceiling**, exact equality for `Exact`, and the failing-direction fixture as the bound for `MeasuredZero`. **`AU6_BUDGETS` is 22 rows**, one per row below (S2); **one** row reuses a constant already counted (row 4) and **five** carry none (rows 8, 13, 16, 18, 19; N4 S6).

| # | Term | Constant | Kind | Budget | Measured | Margin |
| ---: | --- | --- | --- | ---: | ---: | ---: |
| 1 | dialogue over bed | `AU6_INTERVIEW_DIALOGUE_OVER_BED_MIN_LU_HUNDREDTHS` | Floor | **400** | **812** | **2.03×** |
| 2 | duck depth | `AU6_DUCK_DEPTH_MIN_HUNDREDTHS` | Floor | **400** | **1 187** | **2.97×** |
| 3 | voices matched, (a) | `AU6_VOICE_MATCH_MAX_LU_HUNDREDTHS` | Ceiling | **150** | **5** | **30×** |
| 4 | voices matched, (b) | *(row 3's constant)* | Ceiling | **150** | **50** | **3.0×** |
| 5 | dynamics reduced | `AU6_PODCAST_SPREAD_REDUCTION_MIN_HUNDREDTHS` | Floor | **450** | **978** | **2.17×** |
| 6 | programme LRA | `AU6_PODCAST_LRA_MAX_LU_HUNDREDTHS` | Ceiling | **300** | **50** | **6.0×** |
| 7 | normalization prediction | `AU6_NORMALIZATION_PREDICTION_MAX_LU_HUNDREDTHS` | Ceiling | **25** | **1** | **25×** |
| 8 | clipping, every scenario | *(none)* | Exact | **0 runs** | **0** | infinite (measured exactly zero) |
| 9 | gap under the detector | `AU6_LEARN_GAP_BELOW_SILENCE_MIN_HUNDREDTHS` | Floor | **250** | **526** | **2.10×** |
| 10 | repair SNR gain | `AU6_REPAIR_SNR_GAIN_MIN_HUNDREDTHS` | Floor | **500** | **1 142** | **2.28×** |
| 11 | 60 Hz drop | `AU6_HUM_DROP_MIN_DB_HUNDREDTHS` | Floor | **2 400** | **4 970** | **2.07×** |
| 12 | per-harmonic drop, first three | `AU6_HUM_HARMONIC_DROP_MIN_DB_HUNDREDTHS` | Floor | **500** | **1 096** | **2.19×** |
| 13 | clicks after repair | *(none)* | Exact | **0** | **0** (before **12**) | infinite (measured exactly zero) |
| 14 | de-click error drop, node alone against click-free | `AU6_DECLICK_ERROR_DROP_MIN_TENTH_DB` | Floor | **120** | **258** | **2.16×** |
| 15 | speech level retained | `AU6_REPAIR_SPEECH_LOSS_MAX_DB_HUNDREDTHS` | Ceiling | **300** | **101** | **2.97×** |
| 16 | profile band pin | *(none — exact `assert_eq!`)* | MeasuredZero | **0** | **0** | infinite (measured exactly zero) |
| 17 | profile leakage allowance | `AU6_PROFILE_LEAKAGE_ALLOWANCE_TENTH_DB` | Ceiling | **60** | **27** | **2.22×** |
| 18 | room-tone seam | *(none)* | MeasuredZero | **0.0** | **0.0** | infinite (measured exactly zero) |
| 19 | master stem identity | *(none)* | MeasuredZero | **0 samples differ** | **0** | infinite (measured exactly zero) |
| 20 | master passthrough | `AU6_MASTER_PASSTHROUGH_MAX_LU_HUNDREDTHS` | MeasuredZero | **50** | **0** | infinite (measured exactly zero) |
| 21 | delivery deviation, both lanes | `AU6_DELIVERY_DEVIATION_MAX_LU_HUNDREDTHS` | Ceiling | **25** | **0** (worst **1**) | **25×** |
| 22 | true-peak margin, thinnest lane | `AU6_DELIVERY_TRUE_PEAK_MARGIN_MIN_HUNDREDTHS` | Floor | **100** | **204** | **2.04×** |

Plus **`AU6_SOURCE_BUDGETS: [Au6Budget; 3]`** (N4 S7), outside `AU6_BUDGETS` because its rows gate the **sources** and the **two deliveries against each other** rather than one mix: `AU6_AUTHORED_LEVEL_TOLERANCE_HUNDREDTHS` (Ceiling, 25 against 10, 2.5×), `AU6_VOICE_BAND_SEPARATION_BANDS` (Floor, 1 against 3, 3.0×) and `AU6_TARGET_SEPARATION_TOLERANCE_LU_HUNDREDTHS` (Ceiling, 25 against 1, 25×) — all three carry a §11.3 `thresholds` key and a §4.1-shaped row in the manifest (S3), and item 10 asserts them from that array.

Four notes:

1. **Distinctness.** §2.8's assertion list; no AU6 budget may equal a `LoudnessTarget.tolerance_lu_hundredths`, an AU3 fixture budget, or an AU5 fixture budget it could be silently substituted for.
2. **No AU3 or AU5 constant is re-baselined.** AU6 measures its own material against its own constants; the 0.41 dB de-noise headroom and the 2.703× hot-noise margin stand untouched. AU6 writes its own seam and margin helpers rather than reuse ones that hard-code AU5's or AU3's numbers.
3. **Terms measured at or near zero** record `"infinite (measured exactly zero)"` and name their failing-direction fixture as the bound: rows 8, 13, 16, 18, 19 and 20.
4. **Two-sided terms** would use CC7 §4.1 note 5's bracket form. AU6 has none; if probe-2 produces one it is recorded that way rather than forced into a ratio.

### 4.2 Failing directions, same terms

| Term | Failing fixture | Measured |
| --- | --- | ---: |
| dialogue over bed | `au6_a_the_unducked_document_does_not_clear_the_bed` | **−388** |
| duck depth | `au6_a_the_unducked_bed_has_no_depth` | **−14** |
| voices matched, (a) | `au6_a_the_untrimmed_voices_are_not_matched` | **365** |
| voices matched, (a), wrong model | `au6_a_the_nominal_dbfs_trim_is_worse_than_none` | **235** |
| voices matched, (b) | `au6_b_the_raw_trims_do_not_match_the_voices` | **1 442** |
| dynamics reduced | `au6_b_the_bypassed_chain_leaves_the_spread_intact` | **0** |
| programme LRA | `au6_b_the_makeup_less_chain_exceeds_the_loudness_range` | **1 274** (un-trimmed **1 434**) |
| clipping | `au6_a_a_hot_master_clips_and_says_so` | `technical_pass == false` |
| gap under the detector | `au6_c_the_brief_levels_never_reach_the_detector` | ≈ **−3 400**, above −3 500 |
| repair SNR gain | `au6_c_an_unlearned_profile_moves_no_snr` | **132** |
| 60 Hz drop | `au6_c_a_chain_without_the_hum_node_leaves_the_mains_alone` | **1 tenth dB** |
| per-harmonic drop | the same fixture | no harmonic clears **500** |
| clicks / error drop | `au6_c_a_chain_without_the_declick_node_keeps_every_click` | **12** clicks; drop **0.0** (bit-identical stem) |
| speech level retained | `au6_c_an_over_reduced_profile_eats_the_dialogue` | over **300** |
| profile pin | `au6_c_a_one_frame_learn_offset_moves_the_profile` | **29 tenth dB** at band 0 |
| profile bound | `au6_c_a_profile_learned_over_speech_is_not_the_noise_profile` | over the bound |
| room-tone seam | `au6_c_a_one_frame_slip_breaks_the_seam` | > 0.0 |
| master stem identity / programme continuity | `au6_d_a_cut_master_track_is_not_continuous` | diverges at sample **576 000** |
| scratch silent | `au6_d_an_unmuted_scratch_track_reaches_the_mix` | `audible=true`, **−2 965 / −2 968** |
| master passthrough | `au6_d_the_unmuted_scratch_moves_the_master` | **600** (12× over) |
| delivery deviation / true peak | `au6_e_an_unnormalized_export_misses_the_target` | **122** / **778** |
| target separation | `au6_e_two_exports_at_one_profile_do_not_separate` | **0** |
| planner non-vacuity | `au6_b_a_clip_whose_head_window_is_silent_gets_no_fade` | 0 proposed fades |

---

## 5. The agent path — scripted end-to-end tests

`crates/kinewright-agent/tests/mcp_server.rs`, **six** `au6_`-prefixed `#[tokio::test(flavor = "multi_thread")]` tests, one per eval task, following the CC7 templates (`:5422`, `:5724`, `:6151`, `:6420`, `:6721`, `:7080`). Each drives the **real** MCP endpoint with **scripted** tool calls: no LLM, no `AgentDriver`. The helpers `invoke_capability` (`:2032`), `prepare_plan` (`:2050`), `commit_request` (`:2071`), `query_document` (`:2088`) and `cc7_approve_confirmations` (`:2395`) are reused **unchanged**; `resolve_plan_confirmation` (`:2097`) is **not** — it hard-asserts one CC4 description string. The `au6_` namespace is clean at `11a6098`.

### 5.1 Uniform assertions, as they can be written today

1. **Evidence-only planning.** Every planner and inspector response carries `applied: false`, and the document is byte-identical before and after planning. **`evidence_only` is asserted only where it exists**: `get_audio_qc` and `get_audio_repair` publish it (`crates/kinewright-core/src/audio_qc.rs:124`, `crates/kinewright-core/src/audio_repair.rs:133`); the planners publish neither key uniformly, which §13 records rather than papers over.
2. **Revision gating, where it exists.** Two claims and only two:
   - **Commit-time.** A stale `expected_revision` on `prepare_edit_plan` is refused with the document unchanged; likewise on `commit_edit_plan` after a successful prepare.
   - **The two inspectors.** `get_audio_qc` and `get_audio_repair` with a stale `expected_revision` return the typed refusal in `cc7_assert_stale_revision`'s shape (`tests/mcp_server.rs:2284-2305`): `is_error == Some(true)`, `["code"] == "stale_revision"`, `["applied"] == false`, `["evidence_only"] == true`, `["details"]["expected_revision"]`, `["details"]["actual_revision"]`. The stale offset is non-adjacent (`+3`, `+5`, `+7`).

   **`get_audio_levels` is not on this list** — `AudioLevelsArgs` (`crates/kinewright-agent/src/server.rs:12192-12202`) has no `expected_revision`. **Nor is any planner**: all take none and every refusal is `error_text` — prose, no `structured_content`, no `code` (`server.rs:13828-13830`). The probe confirmed it empirically (`plan_dialogue_repair` re-run on a repaired bus returns bare prose; `plan_audio_normalization` with a `profile` argument returns `-32602 "missing field 'track_ids'"`). **CC7 §5.1(2)'s uniform assertion cannot be written for the planners**, so AU6 asserts their refusals **by exact string** (A15) and §13 carries the typed-envelope debt.
3. **One commit, one revision.** Each `prepare_edit_plan` → `commit_edit_plan` pair advances `timeline_revision` **exactly once**. **`au6_a3` has four advances, three of them commits** (§5.3).
4. **Document equality.** The committed document equals `au6_canonical_operations(scenario)` applied to the base document, with the neutrals the planner did not move asserted **absent**, as `cc7_prepare_commit_and_compare` does (`tests/mcp_server.rs:2428-2462`). **The one documented exemption is (c)'s 31 profile rows**, compared as §2.5 states.
5. **The manifest agrees with the document.** The same integers are re-read from `get_audio_levels` and, for (c), `get_audio_repair` — whose `snr` and `click_count` the probe measured agreeing **exactly** with the planner's own numbers.

### 5.2 The six scripts

| Test | Tool-call script | Codes / strings asserted |
| --- | --- | --- |
| `au6_a1_the_interview_ducks_the_bed_and_matches_the_voices` | `get_audio_levels` → `set_track_mix` ×2 + `upsert_audio_bus` ×2 through prepare/commit → `plan_audio_ducking`{`music_track`, `dialogue_tracks`, pinned `depth/attack/hold/release`} → prepare/commit → `get_audio_levels` → `get_audio_qc` | `stale_revision` on `get_audio_qc` |
| `au6_a2_the_podcast_chain_matches_the_voices_and_tames_the_ride` | `set_track_mix` ×2 + `upsert_audio_bus` ×2 through prepare/commit → `plan_audio_normalization`{`track_ids`, `target_lufs_hundredths`} at **both** targets (evidence only, **not committed**) → `plan_clip_fades`{`tracks`} → prepare/commit → `get_audio_levels` → `get_audio_qc` | `stale_revision`; the `plan_audio_normalization` schema refusal by exact string |
| `au6_a3_the_location_dialogue_is_repaired_and_its_gap_filled` | **`start_isolated_with_exporter_and_project_path`** → `split_clip` ×2 (**late-to-early**) + `delete_clip` through prepare/commit → **wait `SilenceStatus::Ready`** → `capture_room_tone`{`expected_revision`, `asset_id`, `source_start_frame`, `source_end_frame`} under an approver → `plan_room_tone_fill`{`track`} → prepare/commit → **wait `SilenceStatus::Ready` again** → `get_audio_repair` (before) → `plan_dialogue_repair`{`tracks`} → prepare/commit → `get_audio_repair` (after) | the §5.5 table; `stale_revision` on `get_audio_repair`; `plan_dialogue_repair`'s already-repaired prose by exact string |
| `au6_a4_the_multicam_cuts_leave_the_master_audio_untouched` | `split_clip` ×6 (**late-to-early**) + `delete_clip` ×4 + `set_track_mix` ×2 through prepare/commit → `get_audio_levels` → `get_audio_qc` | `stale_revision` on `get_audio_qc` |
| `au6_a5a_the_delivery_lands_on_the_ebu_r128_target` | **`start_with_exporter`** → (a)'s canonical batch (the **12 s** document, B2) → `queue_export`{`expected_revision`, `output_path`, `profile: "source_master"`, `normalize_loudness: true`} → poll `get_export_jobs` to `"completed"`, then `audio_report.limiter_passes == 1` and `audio_verification.technical_pass` | — |
| **`au6_a5b_the_streaming_target_is_reachable_by_the_agent`** (B3) | **`start_with_exporter`** → the identical batch → `get_delivery_profiles` (asserts the `youtube_1080p` entry's `loudness_target.integrated_lufs_hundredths == -1400`, `true_peak_ceiling_dbtp_hundredths == -100`, `resolution 1920×1080`) → `queue_export`{`expected_revision`, `output_path`, `profile: "youtube_1080p"`, `normalize_loudness: true`} **accepted**: `structured_content.job.id`, `job.profile == "youtube_1080p"`, `job.state ∈ {"queued", "running"}` → `get_export_jobs` lists that `id` with that profile → `cancel_export`{`job_id`} → poll `get_export_jobs` to a terminal `state` asserted **`!= "failed"`** (`"cancelled"` normally, `"completed"` tolerated). **No encode is awaited and no `audio_report` / `audio_verification` is asserted**: a 1080p render is 127.6 s at 8 s (probe-2 §3), ≈ 190 s at 12 s, over the 180 s deadline | — |

**`queue_export`'s required arguments and the temp path (S8).** `QueueExportArgs` (`crates/kinewright-agent/src/server.rs:11891-11924`) requires **`expected_revision`** and **`output_path`** beside `profile`; `verify` defaults `true`, `normalize_loudness` and `overwrite` default `false`. The output directory is built from **process id + thread id** — `std::env::temp_dir().join(format!("kinewright-au6-<task>-{}-{:?}", std::process::id(), std::thread::current().id()))` — created before the call and `remove_dir_all`'d at the end, AU3's precedent (`tests/mcp_server.rs:5246-5253`).

**Which tests carry an exporter (N1 Q11).** `au6_a4` no longer needs one — (d)'s delivery leg is cut (A1) — so **`au6_a5a` and `au6_a5b`** use `McpServer::start_with_exporter` (`server.rs:319-334`), the shape `au3_queue_export_normalizes_and_verifies_audio` established (`tests/mcp_server.rs:5242-5412`, `:5256-5258`), with its **180 s** polling deadline, its terminal-state match on `"completed" | "failed" | "cancelled"`, and — for `au6_a5a` only — its `audio_report` / `audio_verification` assertions; `au6_a5b` needs the exporter so `queue_export` is **accepted**, then cancels (B3): `ExportJobRecord` (`crates/kinewright-agent/src/export_queue.rs:107-201`) exposes `id`, `output_path`, `profile`, `state` (`queued | running | completed | failed | cancelled`, `:84-90`), `progress`, `delivery_bit_depth`, `error`, `audio_report`, `audio_verification` and `audio_verification_unavailable_reason`, and `queue_export` returns `{ timeline_revision, job }` (`server.rs:3946-3949`); `cancel_export` (`:3971-3984`) returns the record with `state: "cancelled"` unless the job was already terminal. `au6_a1`, `au6_a2` and `au6_a4` use the three-argument `McpServer::start`.

**`au6_a3` needs a saved project and may have an exporter too (B5).** `McpServer::start_isolated_with_exporter_and_project_path(core, playback, analysis, exporter, project_path)` exists at `crates/kinewright-agent/src/server.rs:424-438`, and `set_project_path` / `project_path_handle` are `:533` / `:528`. Draft v1's claim that "the two requirements never meet in one AU6 test" was **false and is deleted**. `au6_a3` uses `TempDirectory` + `Arc<RwLock<Option<PathBuf>>>`, AU5's idiom (`tests/mcp_server.rs:8688-8698`).

**The confirmation broker.** `capture_room_tone` writes bytes under the project directory and confirms first. `au6_a3` runs `cc7_approve_confirmations(server.confirmations(), "capture_room_tone")` beside the awaited call and closes with `approvals.assert_approved_and_stop("capture_room_tone")`, which asserts both that the confirmation was raised and approved and that no other tool raised one.

### 5.3 (c)'s revision ledger and its two silence waits

**Four advances, three of them commits** (B11 / N2 Q6):

| # | Step | Kind |
| ---: | --- | --- |
| 1 | `au6_c_gap_operations(next_clip_id)` — two `SplitClip` late-to-early plus one `DeleteClip` | **commit** |
| 2 | `capture_room_tone` | **Action-applied `AddAsset`**, its own advance |
| 3 | `au6_c_fill_operations(asset)` — the `AddClip`s `plan_room_tone_fill` proposes | **commit** |
| 4 | `au6_c_repair_operations()` — one `UpsertAudioBus` | **commit** |

Step 2 is **not** a commit: `capture_room_tone` applies its own operation through `apply_operation_analyzing` (`crates/kinewright-agent/src/server.rs:2320-2324`) at `args.expected_revision`, so it can never be part of a `prepare_edit_plan` batch.

**Two `SilenceStatus::Ready` waits, not one.** `dialogue_repair_learn_span` refuses while **any referenced asset** is not `Ready | NoAudio` (`server.rs:9752-9774`), and step 3 introduces a **new** referenced asset — the room-tone capture — on track A1. The script waits before step 2 and **again after step 3**. Each wait is a **poll with a deadline** in `au5_invoke_when_silence_is_ready`'s shape (`tests/mcp_server.rs:8191`), never a fixed sleep, and its failure message names the asset and the status it was stuck in.

### 5.4 M36 bookkeeping — the registry and the served surface are UNCHANGED

- `COMPACT_TOOL_NAMES` stays at **7** (`crates/kinewright-agent/src/runtime.rs:17-25`); `served_surface_is_small_and_keeps_the_internal_registry_discoverable` (`crates/kinewright-agent/src/server.rs:26313`) still reports the served quad **7 / 5 660 / 3 510 / 998** (`:26652-26661`) and the registry triple **1 540 264 / 1 397 156 / 120 458** (`:26638`, `:26642`, `:26646`).
- `INSPECTOR_TOOL_NAMES: [&str; 84]` stays at **84** (`crates/kinewright-agent/src/schema.rs:16`); `operation_tools().len()` stays at **54** and `capability_tool_names().len()` at **138**.
- The pinning test is the existing `cc7_the_agent_surface_is_unchanged_by_this_slice` (`tests/mcp_server.rs:2517`), whose four assertion messages are **extended, never relaxed**: `:2528-2532`, `:2544-2554`, `:2555-2562`, `:2581-2588` each gain one clause naming AU6. The registration-order assertions at `:2589-2656` are untouched, because AU6 adds no name.

**The two ledger paragraphs are appended after the AU5 Part B paragraph** — the running doc comment runs `:2464-2514` and the test begins at `:2517`, so "at `:2514`" would append *inside* AU5's sentence (nit N11):

> AU6 §5.4 Part A adds **no** capability at all — it is an evaluation slice — so the registry holds at 54 + 84 = 138 and the generated count cannot move: AU6 adds no `Operation` variant, no tool and no effect descriptor. The one product change Part A ships is an app-side `MixerChain` arm that emits the **existing** `SetTrackAutomation`, which reaches no schema, and which does not touch core's `AudioChain`. The served quad does not move for the thirteenth consecutive measurement.
>
> AU6 §7 Part B adds none either: the seventh eval suite, its assertion variants and its audio evidence block all live in `crates/kinewright-agent/src/eval.rs` and `src/bin/kinewright-eval.rs`, neither of which is a capability. The served quad does not move for the fourteenth consecutive measurement.

### 5.5 Typed codes and prose refusals AU6 asserts

**AU6 introduces no new typed code.**

| Code | Raised when | Owner |
| --- | --- | --- |
| `stale_revision` | `get_audio_qc` / `get_audio_repair` with a stale `expected_revision` | `crates/kinewright-agent/src/color_scopes.rs:600-609`, rendered `server.rs:14224-14234` |
| `audio_clipping` | §4(a)(4)'s hot-master control, inside `AudioQcReport.exceptions` | `kinewright-core::audio_qc` |

`capture_room_tone` is the **only** audio capability with a typed-error envelope — `lut_tool_error`'s (`server.rs:13840-13854`): `code`, `message`, `details`, `applied`, **and no `evidence_only`**, so one matcher cannot serve both this table and §5.1(2)'s. **A15 fixes the reachable set**; `au6_a3` asserts each in its failing direction:

| Code | `server.rs` | How AU6 reaches it | Probe |
| --- | --- | --- | --- |
| `revision_conflict` | `:13956` (via `lut_revision_conflict`, called `:2121`) | `expected_revision: 999` | measured, `details.{actual_revision, allowed, expected_revision, field, observed, recovery_action}` |
| `capture_refused` | `:2218` | the approver refuses the confirmation | measured, `details.{store_file_written:false, document_changed:false, reason, recovery_action}` |
| `project_not_saved` | `:2127` | the same call on a server started without the handle | input-validation |
| `unknown_asset` | `:2143` | an `asset_id` the document does not carry | input-validation |
| `invalid_source_range` | `:2157` | `source_start_frame >= source_end_frame` | input-validation |
| `room_tone_capture_too_long` | `:2182` | a range over `ROOM_TONE_MAX_CAPTURE_MILLISECONDS = 60_000` (`crates/kinewright-media/src/room_tone_store.rs:62`) | input-validation |

**`core_rejected` is not in this table and cannot be** — its two literals (`server.rs:2369`, `:2383`) are inside `apply_lut_batch`, whose only caller is `convert_legacy_look` (`:2500`), while `capture_room_tone` applies through `apply_operation_analyzing` whose rejection arms are bare `error_text` (`:1422-1428`). N1's list named it; draft v1 escalated it; N1.5 A15 rules the reachable set without it.

**The four planners' prose is pinned by exact string** (A15, N4 S17), each one a `const &str` in the test module beside the call that provokes it, so a wording change fails loudly rather than silently weakening the assertion. Only **three** of the four have a prose *refusal* reachable with valid arguments; `plan_clip_fades` has none, and what AU6 pins for it is its "nothing to propose" **success** prose. `plan_audio_normalization`'s `-32602 "missing field 'track_ids'"` is a JSON-RPC error, not `error_text`, and is listed as such. `au6_the_four_planners_prose_is_pinned_by_exact_string` (§11.2 item 52) holds the four constants:

| Tool | Provocation (valid arguments) | Kind | `server.rs` | `const` |
| --- | --- | --- | --- | --- |
| `plan_audio_ducking` | in `au6_a1`, a **second** call after the ducking plan is committed — the music track now carries a gain curve and `replace` is not passed (rule 125) | `error_text` refusal | `:10556-10559` — `"track {} already carries a gain curve; clear it or pass replace: true"` | `AU6_DUCKING_ALREADY_RIDDEN_REFUSAL` |
| `plan_clip_fades` | in `au6_a2`, the item-51 call on a clip whose head window is silent — every clip skipped | `success_structured` prose, `is_error` absent | `:9205-9209` — `"nothing to propose{threshold_clause}; {} clip(s) were skipped and nothing was prepared"`, with `skipped[*].reason = "the track stem is silent over this clip's head or tail window"` | `AU6_CLIP_FADES_NOTHING_TO_PROPOSE` |
| `plan_room_tone_fill` | in `au6_a3`, a call **before** `capture_room_tone`, on a project with no registered room-tone asset | `error_text` refusal | `:9944-9947` — `"the project has no registered room-tone asset; capture one with capture_room_tone first, or name an existing asset with asset_id"` | `AU6_ROOM_TONE_NO_ASSET_REFUSAL` |
| `plan_dialogue_repair` | in `au6_a3`, a **second** call after the repair is committed, `replace` not passed (probe-1 §7.6) | `error_text` refusal | `:9438-9444` — `"audio bus {} ({}) already carries an AU5 repair prefix ({}); pass replace true to rebuild it"`, rendering as `audio bus 1 (Dialogue repair) already carries an AU5 repair prefix (audio_denoise, audio_hum_removal, audio_declick); pass replace true to rebuild it` | `AU6_REPAIR_ALREADY_REPAIRED_REFUSAL` |
| `plan_audio_normalization` | a `profile` argument | JSON-RPC `-32602`, not prose | `"missing field 'track_ids'"` | asserted as the error code and message in `au6_a2`, not as `error_text` |

§13 carries the typed-envelope debt and records that the reachable refusal set is three, not four.

Recorded, not asserted: the ten `RoomToneStoreErrorCode` wire strings (`room_tone_store.rs:131-149`) reach the agent by **string parsing**, not typed re-render (`server.rs:13926-13945`), falling back to `room_tone_capture_failed`; and `room_tone_capture_too_long` is emitted by **two** mechanisms with different `details.observed` shapes.

---

## 6. The person path

The person path is proven **at the operation-builder level**, not by driving a live `KinewrightApp`. `KinewrightApp::new` is private with one call site (`crates/kinewright-app/src/app.rs:206`, `:2175`), and the crate is a **binary with no `[lib]` and no `tests/` directory**, so every test is an inline `#[cfg(test)] mod tests` and every item is `pub(crate)` at most. **CC7 §6 cites `app.rs:180` / `:1858`; both are stale, and §12.1 records the erratum.**

### 6.1 The six-step pattern, per scenario

CC7 §6's six steps exactly — helper block `crates/kinewright-app/src/inspector_ui.rs:8180-8526`, exemplar `cc7_a_a_person_can_author_the_matched_primary_by_hand` (`:8527-8599`):

1. **Build the base document** — `au6_base_document(scenario)`, reading everything off `au6_spec`; **no §2 literal is restated**.
2. **Seed the state without its values** — `au6_unvalued(canonical)`, derived from the canonical batch, never a second literal.
3. **Paint the real widget headlessly and assert it writes nothing** — asserting `edits.operations().is_empty()` (`InspectorEdits::operations` is `#[cfg(test)]`-only, `inspector_ui.rs:260-263`) and the painted control names through `crate::theme::painted_text` with **exact** string equality.
4. **Drive the builder one control at a time** — `track_mix_operation` / `track_mix_toggle_operation` (`crates/kinewright-app/src/mixer_ui.rs:1369`, `:1381`), `insert_audio_effect` (`crates/kinewright-app/src/mixer_pane_ui.rs:1962`), `set_automation_curve` (`mixer_pane_ui.rs:986-1008`), `room_tone_fill_operations` (`crates/kinewright-app/src/timeline_ui.rs:3160`), `clip_audio_operation` (`crates/kinewright-app/src/inspector_ui.rs:4503`) and `noise_profile_operation` (`crates/kinewright-app/src/app.rs:2647`) — **the last two promoted to `pub(crate)` by §0.2 item 18**. Coalescing: `track_mix_coalesce_key(track)` (`mixer_ui.rs:1396`) for a scalar mix drag, **`track_automation_coalesce_key(track, parameter)`** (`crates/kinewright-core/src/model.rs:579`) for a curve edit (S6) — using the mix key for both would fold a curve edit and a fader drag into one undo entry, the opposite of AU4's rule.
5. **Compare the batch order-insensitively, names and values exactly** — `au6_written` / `au6_expected`.
6. **Feed core one operation at a time and compare documents** — `au6_apply_in_order` calls `apply_batch(document, slice::from_ref(operation))` then `document.validate()` per operation (A11), and asserts `assert_eq!(document, au6_canonical_document(scenario, &canonical))`.

**Both directions are mandatory.** Every test also builds the batch with **one value one code off** — `au6_off_by_one` picks a neighbour inside the control's range — and asserts `assert_ne!` on the batch **and** on the document. **The split-order claim is asserted too** (A10): a batch with the splits ascending is asserted to be **rejected** by core with `SplitOutsideClip`.

| Scenario | Builder(s) | Test |
| --- | --- | --- |
| (a) | `track_mix_operation` ×2, `+ Bus`, and **the new `MixerChain::Track` arm** driving `set_automation_curve` (§6.2) | `au6_a_a_person_can_balance_the_interview_and_ride_the_bed` |
| (b) | `track_mix_operation` ×2, `insert_audio_effect` for the EQ and compressor on Voice B's chain, `clip_audio_operation` for the two fades | `au6_b_a_person_can_match_the_voices_and_build_the_chain` |
| (c) | the timeline blade (late-to-early), the `Room tone` button's `room_tone_fill_operations`, the three repair cards' `insert_audio_effect`, `noise_profile_operation` for the 31 rows | `au6_c_a_person_can_repair_the_dialogue_and_fill_the_gap` |
| (d) | the timeline blade for six splits and four deletes, `track_mix_toggle_operation(MixToggle::Mute)` ×2 | `au6_d_a_person_can_cut_the_angles_and_mute_the_scratch_tracks` |
| (e) | `export_loudness_target` at both profiles (§6.4) | `au6_e_the_export_dialog_carries_both_delivery_targets` |

All five live in `crates/kinewright-app/src/mixer_ui.rs`'s `mod tests`, which is why §0.2 item 18 promotes the two private builders rather than scattering the tests.

### 6.2 The shipped fix — a track `AUTOMATION` target

**The gap, verified.** `MixerChain` is `Bus(&AudioBus) | Master(&AudioMaster)` (`crates/kinewright-app/src/mixer_pane_ui.rs:37-43`); `automation_targets` offers `AutomationTarget::Fader` for those two only (`:889-905`); and **no widget anywhere emits `SetTrackAutomation`** — the only `app.rs` hits are `is_audio_mix_operation` (`:1707`), `operation_status` (`:1919`) and tests. After `plan_audio_ducking` commits, the person sees an `A` chip and a **disabled** track fader whose tooltip says *"…clear the automation in the chain pane to ride the fader directly."* (`AUTOMATED_FADER_TOOLTIP`, `crates/kinewright-app/src/mixer_ui.rs:1000-1002`, painted on track strips because `meters_and_fader` at `:1076-1082` is shared). The chain pane has no track target: **the app's own text points a track-fader user at a pane that cannot help.**

**The route (B1 / N2 Q1).** `MixerSelection` is `Bus(AudioBusId) | Master` with two exhaustive `impl` methods, `const fn chain(self) -> AudioChain` (`mixer_ui.rs:286-291`) and `coalesce_key` (`:299-304`), and `AudioChain` is a **core** enum (`crates/kinewright-core/src/media.rs:1171-1174`) used by engine telemetry, the `Learn profile` seam and core's own automation validator. **AU6 does not add `AudioChain::Track`.** `MixerSelection` gains `Track(TrackId)` and **`chain()` becomes `fn chain(self) -> Option<AudioChain>`**, returning `None` for a track; the four call sites handle `None`. Core is untouched, the change stays **app-only**, and there are 92 `MixerSelection` references to sweep (`app.rs` 11, `mixer_pane_ui.rs` 16, `mixer_ui.rs` 65).

**The change, precisely.**

1. **`mixer_pane_ui.rs`** — `MixerChain` gains `Track(&'a Track, &'a TrackMix)`, with arms in `selection`, `effects` (empty), `sidechain` (`None`), `gain_curve`, `gain_tenth_db`, `gain_range` (`TRACK_MIX_GAIN_MIN..=TRACK_MIX_GAIN_MAX`, `crates/kinewright-core/src/model.rs:556-558`), `set_gain_curve`, `effects_mut` and `effect_mut`.
2. **`AutomationTarget` gains one variant (B2), and this contract says so plainly.** `TrackParameter(&'static str)`, beside `Fader | Node(EffectId, &'static str)`, with the **two-way mapping** `Fader ↔ "gain_tenth_db"` and `TrackParameter ↔ "pan_percent"` onto `SetTrackAutomation.parameter`, whose schema is closed to exactly those two strings (`crates/kinewright-core/src/operation.rs:120-131`; `TRACK_AUTOMATION_PARAMETERS`, `model.rs:565`). Three new arms: `automation_targets` (`mixer_pane_ui.rs:889-905`), `automation_target_label` (`:908`) and `set_automation_curve` (`:986-1008`). N1 forbade a new `Operation`, tool or descriptor — not a new app enum variant — so this is legal, and the deviation is declared here rather than left implicit.
3. **`mixer_ui.rs`** — `MixerChainEdits` (`:323-334`) gains `tracks: BTreeMap<TrackId, TrackMix>` and a `track()` accessor in `bus()`'s shape; `drain_into` (`:374`) gains one loop emitting **`SetTrackAutomation`** for a changed curve under `track_automation_coalesce_key(track, parameter)` and **`SetTrackMix`** for a changed scalar under `track_mix_coalesce_key(track)`, dropping a copy equal to what the document already holds, exactly as the bus loop does.
4. **The track strip reuses `edit_toggle`** (`mixer_ui.rs:918-930`) with `MixerSelection::Track(id)`. No new control, no new layout, **no floating surface** (`docs/DESIGN.md:411-412`, pinned by `mixer_ui.rs:4280`).
5. **`AUTOMATED_FADER_TOOLTIP` is retired.** New wording, normative:
   > `"Automation drives this fader. The number is the parked value and typing sets it; clear the curve in the AUTOMATION section of this strip's chain pane to ride the fader directly."`
   
   `au6_the_retired_tooltip_no_longer_points_at_a_dead_end` asserts the old sentence is **absent** from the crate and the new one names the `AUTOMATION` section.
6. **`docs/DESIGN.md`, two edits in one (S11).** One hard-wrapped paragraph appended to the `### Mixer` AUTOMATION block (`docs/DESIGN.md:451-457`) stating that the section now opens on a **track** as well as a bus or the master, that a track offers exactly the two parameters `TRACK_AUTOMATION_PARAMETERS` publishes, and that the **116-point budget against 120 is unchanged** because the list is still four fixed rows; **and** the clause at `:456-457` — *"the mixer is where a bus or master ride is edited"* — becomes *"the mixer is where **a track, bus or master** ride is edited"*, because §6.2 falsifies it. `the_design_note_states_the_new_mixer_rules` (`crates/kinewright-app/src/mixer_ui.rs:4267`) has **three** `for expected in [..]` loops — six phrases (`:4278-4283`), four tokens (`:4290-4295`; `:4287` is the phrase loop's assert message) and an AU5 §6.5 rule-133 set (the fourth token, `size-mixer-noise-well-height`, sits inside that same loop) — and all three are inventoried; the first gains one phrase pin on the new sentence.

**Cost and boundary.** **300–450 lines**, app-only, 3–5 tests, one DESIGN.md edit. **No new `Operation`** — `SetTrackAutomation` has existed since AU4 Part A. **No new tool, no descriptor, no ledger byte, no core change.** With it, (a)'s person path is the **identical** canonical document.

**The alternatives, recorded and not gated — and now measured (A13).** All three curve owners produce a **bit-identical master and bus stem**, so the person reaches the identical rendered programme by any of them; only the *track* stem differs, and only for the bus route. (i) the Music bus's own `gain_curve`, a different owner object; (ii) an `audio_ducking` node with a sidechain (`INSERTABLE_AUDIO_EFFECTS`, `mixer_pane_ui.rs:1860-1870`); (iii) the bed clip's `audio_gain_curve`. AU6 gates the track curve and records the other two.

### 6.3 The shipped fix — `changed_project_range` learns about mix edits

**The gap.** `changed_project_range` (`crates/kinewright-app/src/edit_diff.rs:26-71`) walks **clips only**, and `clip_content_equal` (`:87-96`) compares nine fields and omits `audio_gain_curve`, so the `EditCard` re-watch cue never fires after the agent balances a mix.

**The rule, normative.** Curve operations — `SetTrackAutomation`, `UpsertAudioBus` with a changed `gain_curve`, a clip with a changed `audio_gain_curve` — cover `min(keyframe.at) .. max(keyframe.at)`, clamped into `0 .. document.duration` exactly as `:66-70` clamps; a **cleared** curve covers the span the **old** curve held. Scalar mix operations — `SetTrackMix`, `SetPanLaw`, `SetAudioMaster`, a bus's scalar fields — cover the whole programme. `clip_content_equal` gains `audio_gain_curve`.

**Cost.** ≈ 40 lines plus the **first audio tests in that module**, which has five and none audio (`:160`, `:166`, `:179`, `:193`, `:218`): `au6_a_track_curve_covers_its_keyframe_span`, `au6_a_scalar_mix_edit_covers_the_programme`, `au6_a_cleared_curve_covers_the_span_it_held`, each with its failing direction.

**`RemoveAudioBus` and bus rename are NOT in AU6**: §13, with the half-day cost.

### 6.4 (e)'s person leg

`KinewrightApp::start_export` builds `ExportSettings` **inline**, hard-coding `video_bitrate: 8_000_000` / `audio_bitrate: 192_000` for every profile (`crates/kinewright-app/src/export_ui.rs:1734-1735`), and is not constructible in a test. The one builder-level fact available:

```text
export_loudness_target(true, aspect) == export_delivery_profile(aspect).loudness_target()
```

for `aspect ∈ { None, Some(Widescreen) }`, plus `export_loudness_target(false, aspect) == None`. `export_loudness_target` is `export_ui.rs:759-764`; AU3's `au3_the_loudness_checkbox_carries_the_profile_target_into_the_job` (`:3420`) is **cited, not duplicated**. `au6_e_the_export_dialog_carries_both_delivery_targets` adds only that the two targets AU6's jobs name are the two the dialog produces and that they differ by `AU6_TARGET_SEPARATION_LU_HUNDREDTHS = 900`. **The toggle builds no `Operation`, and the test says so.** The app/agent bitrate-and-raster divergence goes to §13.

### 6.5 What the person path does not have

No `plan_track_mix`; no `RemoveAudioBus` or bus-rename control; no sync-group editor (which is why (d)'s `SyncGroup` is base-document state); no eyedropper beyond `Learn profile`; no loudness-matched A/B hold; no audio QC pane; no bus solo/mute; no scrub audio. All carried to §13 with costs.

### 6.6 Scorecard consequence, stated measurably

> **5 of 5 scenarios complete by the person through the app's own builders: 4 of 5 to the identical canonical document, and (c) to an equivalent one whose repair nodes carry the app's explicit descriptor neutrals (R61). 6 of 6 tasks by the scripted agent; the model lane is pending the real-harness run.**

Published in `au6_manifest.json` under `scorecard.person_agent_parity` and asserted equal to the count of `Au6PersonPath::Expressible` entries, with `person_identical_document` (4) and `person_equivalent_document` (1) asserted to sum to it. **§6.2 did not slip**, so `au6_spec(Interview).person_path` stays `Expressible` and the fallback sentence below is unused; **R61** is the one place the word "identical" does not hold, and the reason is a shape difference the test enumerates row by row rather than a value the person cannot reach. **R62** records the one value no gesture types: `+ Bus` names a bus after its track's caption and the app has no rename control (§6.5, §13), so the person-path tests supply the canonical name to the same fold the button drives and assert the caption differs from it.

**If §6.2 had slipped**, `au6_spec(Interview).person_path` would become `NotApplicable { reason }` naming `MixerChain`, and the sentence would become *"4 of 5 … with the interview's ducking ride reachable only through the bus curve or a sidechain node"* — a checked fact, and one the probe has already shown produces a bit-identical programme.

---

## 7. The model path — eval suite `audio-workflow-v7`

### 7.1 Registration

Four edits in `crates/kinewright-agent/src/bin/kinewright-eval.rs`, all mandatory:

1. `fn audio_workflow_suite() -> Vec<EvalDefinition>` returning **six** definitions, in `color_workflow_suite`'s shape (`:2101-2363`), with `#[allow(clippy::too_many_lines)]`.
2. An `eval_suite` arm (`:230-244`): `"audio-workflow-v7" | "v7" => Ok((AUDIO_WORKFLOW_BENCHMARK_ID, audio_workflow_suite()))`, error string extended. `pub const AUDIO_WORKFLOW_BENCHMARK_ID: &str = "kinewright-audio-workflow-v7";` sits in `eval.rs` beside `COLOR_WORKFLOW_BENCHMARK_ID` (`crates/kinewright-agent/src/eval.rs:2731`).
3. **`is_packaged_benchmark` (`:249-258`) gains it — MANDATORY.** Its own doc (`:246-248`) says why: a packaged suite forgotten here produces no artifacts, no review package, and **overwrites `docs/EVALS.md`**.
4. `usage_text` (`:708`) — the suite list lives inside **one string literal at `:709`**.

### 7.2 Tasks

Task ids are the first whitespace token of `name`; the rule lives in `human_review_template_with_questions` (`crates/kinewright-agent/src/eval.rs:2769-2773`), **not** in `filter_definitions` (`kinewright-eval.rs:260`, which filters by `--only`).

| id | `name` | Fixture builder | Scenario | Deliverable |
| --- | --- | --- | --- | --- |
| `a1` | `"a1 Two-person interview with a music bed"` | `fixture_au6_interview()` | (a) | `SourceMaster`, `normalize_to_profile_target: false` |
| `a2` | `"a2 Podcast with uneven voices"` | `fixture_au6_podcast()` | (b) | `SourceMaster`, `false` |
| `a3` | `"a3 Noisy location dialogue"` | `fixture_au6_location_dialogue()` | (c) | `SourceMaster`, `false` |
| `a4` | `"a4 Event multicam with a master audio track"` | `fixture_au6_multicam()` | (d) | `SourceMaster`, `false` |
| `a5a` | `"a5a Encoded delivery at the EBU R128 target"` | `fixture_au6_interview()` | (e) | `SourceMaster`, **`true`** |
| `a5b` | `"a5b Encoded delivery at the streaming target"` | `fixture_au6_interview()` | (e) | `Youtube1080p`, **`true`** |

`a4`'s deliverable is `normalize_to_profile_target: false` because **(d)'s delivery leg is cut** (A1): the task still produces a review artefact — it must, to have a blind entry — but no AU6 assertion reads its loudness.

**Every task carries a deliverable with `require_audio: true`**, read at `eval.rs:2281`. `human_review_template_with_questions` **skips any result with no deliverable** (`eval.rs:2766-2768`), so without one a1–a4 would have no blind entry at all. **The `proof.png` is a still of a flat 320×180 test raster and is of no use to a listener**; it is kept because `cc7_is_blind_media_name` and the `listing.len() == artefacts + 1` shape require it, and the README says so.

Every builder imports its media from `kinewright_media::au6_sources` and returns a `PreparedFixture` (`eval.rs:789-843`) whose `project_path` is `None` **except `fixture_au6_location_dialogue()`**, which saves into a `TempDirectory` moved into `PreparedFixture::new`'s `resources` — its `Drop` removes the directory (`crates/kinewright-media/src/test_support.rs:44-48`) — exactly `fixture_cc7_log_like`'s pattern (`kinewright-eval.rs:2500-2530`).

### 7.3 Prompts

One user turn per task (`max_turns: 1`). Each names the tracks and the intended outcome and **never names a parameter or a value**.

- **a1** — *"Tracks 2 and 3 are the two people in this interview and track 4 is the music bed under them. Bring the two voices to one comfortable level, group them so I can ride them together, and get the bed out of the way whenever either of them is talking without making it pump."*
- **a2** — *"This podcast has two guests recorded on different microphones: one is quiet and steady, the other is loud and swings a lot. Bring them to one level, tame the loud one's swing so it stops jumping out, and clean up the hard edit in the middle so it does not click."*
- **a3** — *"This location dialogue was recorded next to a generator. There is hiss, a mains hum and some clicks, and a chunk in the middle had to come out. Fill the hole with the room's own tone, then clean the noise, the hum and the clicks off the dialogue without hollowing out the voice."*
- **a4** — *"This event was shot on two cameras and recorded on a separate audio recorder. Cut between the two angles at the three moments the action changes, mute the cameras' own scratch audio, and leave the recorder's track completely alone."*
- **a5a** — *"Deliver this interview as a broadcast master at the standard programme loudness, and tell me what it measured."*
- **a5b** — *"Deliver this interview for a streaming platform at that platform's loudness, and tell me what it measured."*

### 7.4 Budgets

`fn audio_workflow_budget(max_tool_calls: u32) -> EvalBudgets`, in `color_workflow_budget`'s shape (`kinewright-eval.rs:2062-2072`).

| Task | `max_tool_calls` | `max_operations` | `max_undos` | `max_wall_time` | Rationale |
| --- | ---: | ---: | ---: | ---: | --- |
| a1 | 16 | 8 | 8 | 15 min | levels + two trims + two buses + a duck plan + prepare/commit + QC |
| a2 | 16 | 8 | 8 | 15 min | two trims + two buses with two nodes + two normalization reads + a fade plan |
| a3 | **24** | **16** | **16** | **25 min** | a split pair, a delete, a capture with its confirmation, a fill, a repair plan, two `get_audio_repair` reads, two silence waits |
| a4 | **24** | **16** | **16** | 15 min | six splits, four deletes, two mutes; **no encode** |
| a5a | 12 | 4 | 4 | **25 min** | one **12 s** export at 320×180 (probe-1 §5: 21 005 + 1 419 ms) and one `get_export_jobs` poll; the wall time is the encode's |
| a5b | 12 | 4 | 4 | **25 min** | as a5a, at **1920×1080** — the one lane that renders the profile raster, ≈ 190 s projected from probe-2's 127.6 s at 8 s; hand-run (B3) |

`max_tokens: 60_000`, `max_cost_usd: Some(2.00)`, `max_turns: 1` throughout; **`max_undos` equals `max_operations`** at every task (CC7 §0.3 F-E2). `SessionConfig.max_turns` is assigned at **`eval.rs:1402`** as `budgets.max_tool_calls + 2`; `budgets.max_turns` is only *scored* (`eval.rs:3469`, `:4671-4672`) and never enforced on the session (nits N3, N4).

### 7.5 The new `EvalAssertion` variants, and `is_audio_assertion`

`EvalAssertion` has **57** variants at `11a6098` (`crates/kinewright-agent/src/eval.rs:175-518`), eight of them colour. AU6 adds **six** and reuses one:

| Variant | Fields | Reads |
| --- | --- | --- |
| `DeliveryAudioVerified` | `profile: DeliveryProfile, maximum_deviation_lu_hundredths: i32, minimum_true_peak_margin_hundredths: i32` | `outcome.audio.verification` |
| `DialogueOverBedAtLeast` | `dialogue_bus: AudioBusId, music_bus: AudioBusId, window: Range<TimeCode>, minimum_lu_hundredths: i32` | **`outcome.audio.level_reports`** |
| `VoicesMatchedWithin` | `first: MixSpectrumPoint, first_window: Range<TimeCode>, second: MixSpectrumPoint, second_window: Range<TimeCode>, maximum_lu_hundredths: i32` | **`outcome.audio.level_reports`** |
| `TrackSilentInMix` | `track: TrackId` | `outcome.audio.track_levels` — asserts **`integrated.is_none()`**, exact, no budget. **Not `!audible`** (A19): a video-only track reports `audible: true, integrated: None` |
| `RepairSnrGainAtLeast` | `point: MixSpectrumPoint, minimum_db_hundredths: i32` | `outcome.audio.repair_before` / `repair_after` |
| `AudioQcTechnicalPass` | `range: Range<TimeCode>` | `outcome.audio.qc.technical_pass` |
| **`ProgramAudioContinuous`** *(existing, reused)* | `track: TrackId, asset_alias: String` | the final document (`eval.rs:435-441`, evaluated `:6821`) |

**`DialogueOverBedAtLeast` and `VoicesMatchedWithin` read `level_reports`, not `window_levels` (B7).** §4(a)(1)/(a)(3)/(b)(1) measure those terms with **`mix_levels`**, and reading a different instrument in the model lane would assert a different quantity against the same constant — the S8 violation §10.2 forbids.

**`NoClippingInQc` and `MasterTrackUntouched` are not added** — the first becomes `AudioQcTechnicalPass`, the second duplicates `ProgramAudioContinuous`.

**`is_audio_assertion`, beside `is_color_assertion`, not instead of it.** `is_color_assertion` (`eval.rs:4097-4163`) is a `const fn` with a deliberately **exhaustive** match. AU6 adds a twin in the same shape and gates on the **disjunction**, keeping the runner's existing `extend` (S21):

```rust
let mut measurements = definition
    .assertions
    .iter()
    .filter_map(|assertion| assertion_measurement(assertion, outcome))
    .collect::<Vec<_>>();
measurements.extend(ungated_color_measurements(outcome));
```

where `assertion_measurement` tries `color_measurement` then `audio_measurement`. **`cc7_only_colour_assertions_emit_measurements` (`eval.rs:11131-11142`) is untouched.** The twin `au6_only_audio_assertions_emit_audio_measurements` asserts the same three facts about `audio_measurement` **and** that the two classifiers are **disjoint over all 63 variants** (57 + AU6's six).

**Thresholds are variant fields and the suite call site reads a constant**; `au6_every_audio_assertion_threshold_is_an_au6_scenarios_constant` asserts it.

### 7.6 The audio evidence block

`evaluate_assertion` (`eval.rs:3668-3672`) sees only `&EvalDefinition` and `&EvalOutcome`, and `EvalOutcome` (`:879-898`) carries no `Analysis`, no exporter and no tool log; `PreparedFixture`'s handles are dropped before `evaluate` runs. Six of the seven variants need a **measurement**, so an arm in `evaluate_assertion` would be unreachable.

1. **Measure inside `run_eval_with_artifacts`**, immediately after the deliverable step and **before** `restore_original` — the colour block sits at `:1461` and the restore at `:1467`, with the comment naming the reason. `measure_audio_block` is inserted **beside** `measure_color_block`.
2. **Carry it on `EvalOutcome`:**

```rust
pub struct EvalOutcome { /* … unchanged, incl. color … */ pub audio: Option<AudioEvalEvidence> }

pub struct AudioEvalRequest {
    pub range: Range<TimeCode>,
    pub window_milliseconds: u32,               // AU6_WINDOW_MILLISECONDS
    pub hop_milliseconds: u32,                  // AU6_HOP_MILLISECONDS
    pub level_windows: Vec<(&'static str, Range<TimeCode>)>,   // §2.3's named windows
    pub window_points: Vec<(&'static str, MixSpectrumPoint)>,
    pub repair_point: Option<MixSpectrumPoint>,
    pub qc_range: Option<Range<TimeCode>>,
    pub delivery_verification: Option<DeliveryProfile>,
    pub silent_tracks: Vec<TrackId>,
}

pub struct AudioEvalEvidence {
    pub level_reports: BTreeMap<&'static str, MixLevelReport>,
    /// B7: keyed by a point LABEL — "master", "bus:dialogue", "bus:music", "bus:dialogue_repair",
    /// "bus:voice_b", "track:<n>" — because `MixSpectrumPoint` derives no
    /// `Ord`/`PartialOrd`/`Hash` (`kinewright-core/src/media.rs:1242-1253`)
    /// and AU6 adds no core derive.
    pub window_levels: BTreeMap<&'static str, MixWindowLevelReport>,
    pub track_levels: BTreeMap<TrackId, (bool, Option<i32>)>,   // (audible, integrated)
    pub repair_before: Option<AudioRepairReport>,
    pub repair_after: Option<AudioRepairReport>,
    pub qc: Option<AudioQcReport>,
    pub verification: Option<DeliveryAudioVerification>,
    pub errors: Vec<AudioEvidenceError>,
}

pub struct AudioEvidenceError { pub quantity: AudioEvidenceQuantity, pub message: String }
pub enum AudioEvidenceQuantity { Levels, WindowLevels, TrackLevels, Repair, Qc, DeliveryVerification }
impl AudioEvalEvidence { pub fn unmeasurable_reason(&self, quantity: AudioEvidenceQuantity) -> Option<String>; }
impl AudioEvalRequest { pub fn from_assertions(assertions: &[EvalAssertion]) -> Option<Self>; }
```

Every shape mirrors the colour block's: `ColorEvalRequest::from_assertions` `eval.rs:607-662`, `ColorEvalEvidence.errors` `:712`, `ColorEvidenceQuantity` `:721-731`, `unmeasurable_reason` `:745-759`. **The `errors` vector is what lets an assertion tell "not asked for" from "asked for and unmeasurable"**, so an unmeasured quantity fails **its own** assertion with a named reason; `au6_a_delivery_verification_without_a_deliverable_is_recorded_as_an_error` asserts both directions.

3. **`EvalDefinition` gains a plain `audio: Option<AudioEvalRequest>`** beside `color` (`:87-103`), edited into **all twenty** definition literals in `kinewright-eval.rs` (`:1081` … `:2346`) and **all six** in `eval.rs`'s tests (`:9609`, `:9655`, `:10597`, `:10643`, `:11853`, `:11932`) as `audio: None` — **26** sites (N4 S10). `EvalDefinition` has **no derives at all**.

**Size: 400–600 lines, and its own implementer** (§12, implementer **P**).

### 7.7 `EvalDeliverableSpec.normalize_to_profile_target`

A **plain field with no `serde` attribute**: `EvalDeliverableSpec` derives `Debug, Clone, Copy, PartialEq, Eq` and nothing else (`eval.rs:105-125`), and the `benchmarks/auto-edit/v*/manifest.json` files are hand-written JSON this struct never serialises, so no field addition can move their bytes. CC7 ruled the same point for `delivery_bit_depth` (`docs/CC7-WORKFLOW-EVALUATION.md:1165`).

**Every live literal, twelve sites:** `crates/kinewright-agent/src/bin/kinewright-eval.rs` **827** (`rerender_document`), **1395** (`finished_cut_suite`), **1431** (`event_multicam_definition`), **1563** (`music_montage_definition`), **1874** (`editorial_cut_suite`), **1995** (`generalization_suite`), **2079** (`color_workflow_deliverable`'s body; `:2078` is the signature); `crates/kinewright-agent/src/eval.rs` **10415**, **10449**, **10531**, **10656**, **12003**. All twelve take `false`. **No `Default` impl, no `..Default::default()`.**

**What it does.** `export_and_probe` builds settings at `eval.rs:2217-2221` through `spec.profile.export_settings(...)`, which hard-sets `loudness_normalization: None` (`crates/kinewright-core/src/delivery.rs:182-185`) — **every eval export in the tree today runs un-normalized**. With the flag set, `export_and_probe` assigns `settings.loudness_normalization = Some(spec.profile.loudness_target())` after building the settings, then the deliverable path calls `verify_delivery_audio` and stores the `DeliveryAudioVerification`. This is the **only** change to the shared runner's export path, it is off at every existing literal, and `au6_v6_deliverables_are_byte_identical_without_normalization` asserts it.

### 7.8 The exporter on the eval server

`run_eval_with_artifacts` starts the server with the three-argument `McpServer::start` (`eval.rs:1389-1394`), so an agent under eval **cannot `queue_export`**. `a5a` and `a5b` need it. AU6 changes that one call to `McpServer::start_with_exporter(fixture.core.clone(), Arc::clone(&fixture.playback), Arc::clone(&fixture.analysis), Arc::clone(&fixture.exporter))` (`crates/kinewright-agent/src/server.rs:319-334`). `PreparedFixture` already carries `exporter` (`eval.rs:794`) built from the same `Arc<T>` (`:829-831`), so **no fixture changes**, and `apply_fixture_project_path` (`:1507-1511`) still supplies the saved project for `a3` through `set_project_path` (`server.rs:533`).

**This is a shared-runner change and affects all six existing suites**, and is stated as such; no v1–v6 prompt names `queue_export`. `au6_the_eval_server_carries_an_exporter` asserts both directions.

### 7.9 Published benchmark

`benchmarks/auto-edit/v7/` with **three** files and no fixture bytes, v6's shape.

- **`manifest.json`** — v6's twelve top-level keys in file order: `schema_version` (**7**), `benchmark_id`, `title`, `status` (`"pending_real_harness_run"`), `runner`, `implementation`, `contract`, `authority` (`"kinewright_core::au6_scenarios"`), `fixture_provenance`, `score_layers`, `acceptance_target`, `tasks[]` (six). **`acceptance_target` has exactly six keys and the sixth is `note`** (S20): `samples_per_scenario: 3`, `minimum_machine_passes_per_scenario: 3`, `minimum_human_accepts_per_scenario: 2`, `minimum_mean_human_rating: 4.0`, `every_question_answered: true`, `note`. Each task carries `id`, `scenario`, `name`, `fixture`, `ground_truth`, `prompt`, `human_question`, `delivery` (with `normalize_to_profile_target` beside `profile` / `delivery_bit_depth` / `focus_*` / `proof_*` / `require_audio`), `budget` (`turns`, `tool_calls`, `operations`, `tokens`, `cost_usd`, `wall_time_ms`, `undos`) and `machine_assertions`.
- **`ground-truth.json`** — the canonical document per scenario plus the expected measurements, **generated** by `au6_ground_truth_json()` in `kinewright-eval.rs`'s test module (core is data-and-arithmetic; a JSON writer is neither), with byte equality asserted and **no environment knob**.
- **`README.md`** — v6's structure section by section, stating plainly that the reviewer's artefact is the `.mp4` they **listen to** and that the `proof.png` is a still of a flat test raster.

**`HumanQuestion.id`'s doc comment** (`crates/kinewright-agent/src/eval.rs:1071`, "The scenario letter this question belongs to") is edited by P to "the scenario letter (v6) or the base task id (v7)", because §8.2 keys AU6's questions by base task id (N4 N13).

**`published_v7_manifest_tracks_the_audio_workflow_suite`** asserts, both directions: task ids; `machine_assertions` counts; every `budget` field; every `prompt`; every `delivery.*` **including `normalize_to_profile_target`**; every `human_question` against `AU6_QUESTIONS`; and each ground-truth document against `au6_canonical_operations` applied to its base document. **It also asserts every `machine_assertions` string is non-empty, distinct within its task, and names either an `au6_scenarios` constant or a tool in `capability_tool_names()`** — the weakest check that would have caught v5's `g3` drift. Retrofitting v1–v6 is §13.

### 7.10 CI and the real-harness run

CI is two jobs (`windows`, `linux`), each `cargo fmt --check`, `cargo build --workspace`, `cargo test --workspace`, `cargo clippy --workspace --all-targets -- -D warnings`, with **no `KINEWRIGHT_EVAL`, no `--ignored`, no `timeout-minutes` and no GPU job**. CI covers the suite's **unit** tests: construction, assertion dispatch, the evidence block's both-direction plumbing test, packaging, the leak test and `published_v7_...`. **Running v7 against a real harness is Riel's action**, as v6's is; `docs/EVALS.md` records "pending real-harness run". **There is no published v6 result to compare against** and AU6 does not pretend otherwise.

---

## 8. The blind review package

### 8.0 What "blind" means here, and what it does not

**Definition, carried from CC7 §8.0 unchanged.** A blind reviewer sees **the artefact and the scenario's question, and nothing that says how the machine judged it or which run, sample, harness or model produced it.**

**What is deliberately NOT blinded: the scenario identity.** It is inherent in the question, and a two-voice-plus-bed programme is recognisable in three seconds. What the package *does* deliver is that the reviewer cannot see the task id, sample index, run id, benchmark id, harness, model, machine verdict, assertion name, or any parameter the agent chose.

**One audio-specific limit.** There is no loudness-matched A/B hold in the product (§13), so a1's and a5b's artefacts are heard at whatever level the reviewer's player produces. The README asks for **one listening level for the whole package** and says why.

### 8.1 Schema 2 is reused unchanged

`HumanReviewFile` (`crates/kinewright-agent/src/eval.rs:1013-1023`), `HumanTaskReview` (`:1046-1066`), `HumanQuestion` (`:1069-1077`), `BlindReviewForm` (`:1098-1102`), `BlindReviewEntry` (`:1104-1114`), `BlindKeyFile` / `BlindKeyEntry` (`:1120-1135`) and `HUMAN_REVIEW_SCHEMA_VERSION = 2` (`:1026`) are untouched; `blind_id` is still the first `BLIND_ID_HEX_LENGTH = 12` hex characters of `artifact_sha256` (`:1031-1044`). `HumanRatingDimension` stays at six.

### 8.2 The audio arms, in both hard-coded branches

1. **`review_questions`** (`crates/kinewright-agent/src/bin/kinewright-eval.rs:368`) returns early when `benchmark_id != COLOR_WORKFLOW_BENCHMARK_ID` (`:370-372`) and, for colour, keys by **scenario index** (`format!("c{}", index + 1)`, `:379`). **AU6's six tasks come from five scenarios, so the arm cannot be CC7's shape verbatim** (nit N9): the audio arm keys by **base task id** (`a1`, `a2`, `a3`, `a4`, `a5a`, `a5b`) directly from `AU6_QUESTIONS`, and **`HumanQuestion.id` is the base task id**, not a scenario letter — `a5a` and `a5b` carry `id: "a5a"` / `"a5b"` over the same prompt string. The contract states it because CC7's field doc says "the scenario letter".
2. **`human_review_template_with_questions`** (`eval.rs:2757-2762`) picks `not_applicable` by benchmark id at `:2781-2787`. It gains:

```rust
pub const AUDIO_WORKFLOW_NOT_APPLICABLE: [HumanRatingDimension; 4] = [
    HumanRatingDimension::Story,
    HumanRatingDimension::Pacing,
    HumanRatingDimension::VisualFinish,
    HumanRatingDimension::Captions,
];
```

so `accepted` requires **`AudioFinish` + `DeliveryReadiness`** plus every question answered — `COLOR_WORKFLOW_NOT_APPLICABLE`'s exact mirror (`:2733-2741`). `summarize_human_review`'s schema-2 rule is unchanged and already bites (`:2925-2937`).

### 8.3 What enters the package

One entry per **artefact**. All six tasks carry a deliverable, so all six contribute; `blind_review_form` sorts by `blind_id` and **dedupes one entry per distinct `blind_id`** (`eval.rs:2834-2835`), so two byte-identical artefacts share one viewing (`docs/EVALS.md:84-86`). **a5a and a5b are two distinct encodes of one document and therefore two `blind_id`s** — which is why (e) is two tasks.

### 8.4 The six questions, verbatim

One clause each, in the row's two words — **balance** and **intelligibility**.

| id | Question |
| --- | --- |
| `a1` | "Is the dialogue clearly balanced above the music bed?" |
| `a2` | "Are both voices at one comfortable level?" |
| `a3` | "Is the dialogue intelligible?" |
| `a4` | "Is the programme audio at one level across the angle changes?" |
| `a5a` | "Is this delivery at a comfortable listening level?" |
| `a5b` | "Is this delivery at a comfortable listening level?" |

These live in `au6_scenarios::AU6_QUESTIONS` and are asserted equal to the manifest's `human_question` fields and to `review_questions`' output. **a5a and a5b carry the same sentence deliberately**: the same judgement about two files, answered twice without being told which target each is.

### 8.5 The leak test

**`au6_the_blind_package_discloses_no_machine_provenance`** builds a package from synthetic results and scans **both** the `blind/` listing **and the serialised bytes of `blind/review-form.json`**, asserting neither contains:

- the run id or the benchmark id;
- `CC7_MACHINE_PROVENANCE_NEEDLES` (`kinewright-eval.rs:6348-6350`: `"-sample-"`, `"agent"`, `"person"`, `"model"`, `"harness"`, `"passed"`, `"assert"`), reused verbatim;
- any **`au6_canonical_parameter_names()`** entry, derived from `AU6_SCENARIO_SPECS`' canonical operations as `cc7_canonical_parameter_names` derives its set (`kinewright-eval.rs:6360-6385`);
- any **value needle of three or more digits**, derived from `au6_scenarios`' canonical values as `cc7_leak_value_needles` does (`:6412-6424`);
- a task id **only in `"task_id": "a1"` / `"task_id": "a5a"` / `"task_id": "a5b"` key form** (S13 / N2 Q9). **Bare `a1`..`a4` are not needles**: they are valid two-character lowercase hex, and the scanned surface is a listing of `<12 hex>.png|.mp4` names plus a form carrying those same `blind_id`s, so a bare needle would contradict the set's own "≥ 3 digits" rule and match hash prefixes. CC7 never scanned for task ids at all.

**And nothing that appears in a question.** `au6_leak_needles_never_appear_in_a_question` asserts, as its own claim, that no needle is a substring of any `AU6_QUESTIONS` entry — the assertion that stops the brief's original mistake from returning.

It also asserts the listing length equals the artefact count plus one, that every media name matches `^[0-9a-f]{12}\.(png|mp4)$`, and that `blind-key.json` is **not** inside `blind/`. *Failing directions:* a leaked copy named `a1-sample-1.mp4` in `blind/`, **and** a form entry carrying `"task_id": "a1"` — both asserted to trip the check.

### 8.6 The human gate, M40's wording

> each passes **3/3** model samples against its technical gates; at least **2 of 3** outputs are accepted by a person; **every question is answered**; and the mean human rating is at least **4.0/5** over the applicable dimensions, N/A dimensions excluded. **One success does not satisfy the slice.**

### 8.7 Scorecard outcomes, stated measurably

- **Objective eval coverage versus subjective review time** — `objective_assertion_count` against `human_question_count` (**1** per task).
- **Workflows completed end to end by both person and agent** — 6/6 agent, 5/5 person (§6.6), as `scorecard.person_agent_parity`.
- **Generalization** — five scenarios × the named axes × two CI operating systems, as `scorecard.generalization_matrix`, with **uncovered cells published as uncovered**: real speech, real music, a real room, a multicam *encode*, and any cross-OS decoded delta are all `false`, and §13 says why.

---

## 9. Serialization and migration

1. **Pre-AU6 documents load unchanged.** AU6 adds no `Document`, `Track`, `TrackMix`, `AudioBus`, `AudioMaster`, `Clip`, `Effect`, `MediaAsset`, `SyncGroup` or `ExportSettings` field, and changes no effect name, parameter name, unit or default; `effect_documentation()` does not move a byte.
2. **`EvalResult.measurements` is unchanged.** AU6 emits audio measurements through the existing `Vec<EvalMeasurement>` (`crates/kinewright-agent/src/eval.rs:913-921`, `skip_serializing_if`). `EvalResult` derives `Serialize` only and `results.jsonl` is write-only, so a result with no measurements serialises **byte-identically to today**.
3. **`EvalDeliverableSpec.normalize_to_profile_target` is a compile-time change**, edited at all twelve literals to `false`. No `Default`, no `..Default::default()`.
4. **`EvalDefinition.audio` and `EvalOutcome.audio` are internal**; neither struct is serialized, so both are ordinary field additions with **26** literal edits to `audio: None` (20 in `kinewright-eval.rs`, 6 in `eval.rs`'s tests).
5. **`benchmarks/auto-edit/v1..v6` are byte-unchanged.** Those manifests are hand-written JSON no Rust struct serialises.
6. **`human-review.json` stays at schema 2** — no version bump, no new field, no new `HumanRatingDimension`; only `not_applicable`'s contents differ, and that is data.
7. **`blind/review-form.json` and `blind-key.json` stay at `schema_version: 1`**; `--score-review` behaves identically for a v6 and a v7 run.
8. **`ExportSettings`, `ExportJobRecord`, `ExportAudioReport`, `DeliveryAudioVerification`, `AudioQcReport`, `AudioRepairReport`, `NoiseProfileReport`, `MixWindowLevelReport` and the core `AudioChain` enum are unchanged.** AU6 reads them all and adds no field and no variant.
9. **`au6_manifest.json` is `manifest_version: 1`**, a test fixture, not a persisted user artefact.
10. **The two behavioural changes to shared paths are additive and named**: §7.8's exporter on the eval server, and §7.7's normalization flag, off at every existing literal.
11. **The app-side enum additions are `pub(crate)` and reach no wire.** `MixerSelection::Track`, `MixerChain::Track` and `AutomationTarget::TrackParameter` are all app-private; the only thing that leaves the app is the **existing** `SetTrackAutomation`.

---

## 10. Ordering and determinism

1. **Every gated number is an integer.** LUFS, LU, dBTP and dBFS in `_HUNDREDTHS`; gains and profile bands in `_TENTH_DB`; counts plain; time in `TimeCode` project frames. **No AU6 API returns an `f32`/`f64` to an agent or the UI, and no AU6 constant is a float** — the two exceptions are the `f32` PCM the generators author and the seam, whose instrument is an `f64` sample-amplitude difference asserted **exactly zero** (§4(c)(5)).
2. **One instrument per gate, named.** `mix_levels` is BS.1770-gated over one contiguous named window ≥ 400 ms; `mix_window_levels` is short-window RMS on the aligned 200 ms grid and is explicitly **not** BS.1770 (`crates/kinewright-core/src/media.rs:1367-1370`); `audio_qc` supplies LRA. A gate uses one, never two, and the model lane reads the **same** instrument as the technical lane (§7.5).
3. **The call pattern is part of the contract, not an optimisation.** `mix_levels` once per authored turn; one whole-programme `mix_window_levels` render per mix point, sliced by integer index; `audio_qc` for LRA. It is normative because it is the difference between a 105 s lane and a 300 s one.
4. **Closed-form sampling, no clock, no adaptive stride.** The only polls are (c)'s two `SilenceStatus::Ready` waits and (e)'s export-job polls, both with deadlines and both asserting a terminal state rather than a duration.
5. **Margin definitions.** Floor: `measured / budget ≥ 2`. Ceiling: `budget / measured ≥ 2`. Zero: `"infinite (measured exactly zero)"` with the failing-direction fixture as the bound. Two-sided: CC7 §4.1 note 5's bracket form. **Two helpers, one per direction** (S16), in `src/au6_fixtures.rs` — `au3_fixtures.rs:243-250`'s `margin(observed, allowed)` is `allowed/observed`, a **ceiling** margin only, and it lives in a `tests/` integration file `src/au6_fixtures.rs` cannot reach. Both print `{value:.3}x`, or `"unbounded"` at zero.
6. **No `cfg`-conditioned tolerance, ever.** There is no `cfg(target_os)` anywhere in the audio path and AU6 adds none. A Windows figure inside a raw budget but under 2× margin earns a **per-OS note in the constant's doc comment**, never a per-OS constant (`docs/AU3-LOUDNESS-AND-DELIVERY.md:1240-1243`).
7. **No RNG beyond the seeded LCG.** `pseudo_random_amplitude` (`crates/kinewright-media/src/test_support.rs:417`) is xorshift64 seeded `0x2545_f491_4f6c_dd1d`. `blind_id` is a hash prefix.
8. **No `Display` parsing in a gate.** Typed codes come from `structured_content["code"]`; the four planners' prose (three refusals and one "nothing to propose") is asserted as **exact strings held in named constants**, which is a pin on a shipped wording, not a parse.
9. **Iteration orders.** Scenarios in `AU6_SCENARIOS` order; tasks `a1, a2, a3, a4, a5a, a5b`; tracks and buses in **document order** (what `MixLevelReport` publishes); windows by ascending start frame; channels `[left, right]`; blind entries by `blind_id`.
10. **The generators are pure**, and `au6_wav_round_trip_is_sample_exact` proves the decode agrees. **Encoded output is not asserted bit-identical across platforms**; within one machine AU3 already pins that an un-normalized export is byte-identical (`crates/kinewright-media/tests/au3_fixtures.rs:546`).
11. **`au6_scenarios` has no I/O**, so two evaluations produce identical values on both operating systems, and the manifest asserts the module's constants equal the manifest's numbers rather than the reverse.
12. **Summation order is the product's.** Parity is the mix-buffer contract at `1e-6` (`assert_playback_matches_export`, `crates/kinewright-media/src/audio.rs:5968-6010`), which AU6 **cites and does not re-assert**. The one bit-exact claim AU6 makes is over **stems on one machine** (§4(a)(5), §4(d)(1)), which the probe measured at `max |Δ| = 0.000e0`.

---

## 11. Exit fixtures and numeric gates

`crates/kinewright-media/src/au6_fixtures.rs` (`mod au6_fixtures;` in `crates/kinewright-media/src/lib.rs`, `cfg(test)`), `crates/kinewright-media/tests/fixtures/au6_manifest.json`, `crates/kinewright-core/tests/au6_core.rs`, agent cases in `crates/kinewright-agent/tests/mcp_server.rs`, eval unit tests in `crates/kinewright-agent/src/eval.rs` and `src/bin/kinewright-eval.rs`, and inline app cases.

**`au6_fixtures.rs` is in `src/`, and that is load-bearing** (A9/E9): `mix_audio_stems` and `MixStems` are `pub(crate)` (`crates/kinewright-media/src/export.rs:1099`, `:999`), so §4(a)(5)'s and §4(d)(1)'s stem gates are unreachable from `tests/`. AU6 does **not** widen the public surface. All AU6 media lanes live in that one file so there is one shared engine and one `performance` measurement.

### 11.0 Fixture-quality rules

**CC6 §11.0.1–9 are carried forward verbatim** (`docs/CC6-QC-AND-MANAGED-DELIVERY.md:1352-1362`), as CC7 §11.0 carries them. The four that bite hardest:

- **11.0.1 — expected values are analytic or independently transcribed.** **No AU6 fixture may obtain an *expected value* by calling `mix_levels`, `mix_window_levels`, `mix_noise_profile`, `audio_qc`, `audio_repair`, `verify_delivery_audio`, the loudness meter, the limiter, or any planner.** The **source-content exemption** is explicit: `au6_sources` **must** call `test_support::tone` and `pseudo_random_amplitude` to *author* material. **Three regression pins are labelled as pins and are the named exceptions**: `AU6_A_DUCK_KEYFRAMES`, `AU6_C_LEARN_PROJECT_RANGE` and `AU6_C_LEARNED_PROFILE_TENTH_DB` are all **committed output of a shipped detector or planner**, transcribed once from a measured run, and their claim is *"the product still does what it did when this was measured"*, never *"this is what the authored material implies"*. §2.5 states the one analytic bound that **is** derivable and §13 carries the closed-form percentile derivation with a cost.
- **11.0.5 — a check that cannot fail is a defect.** Every §4 gate names its failing-direction fixture and §4.2 tabulates them with the probe's measurements.
- **11.0.6 — no environment gate.** No AU6 file may name `KINEWRIGHT_GPU_TESTS_MAY_SKIP`, `KINEWRIGHT_AUDIO_TEST`, `fixture_gpu_or_skip` or any TTS variable, and none may contain `cfg(target_os` (§11.4). **AU6 opens no audio device.**
- **11.0.3 — manifest thresholds are asserted equal to the code constants.**

### 11.1 Sources

1. **(a)** three audio buffers (Voice A, Voice B, chord bed) at 300 frames, plus the **video-only** picture `au6_picture_source(300)` on V1 (B1).
2. **(b)** two audio buffers at 300 frames.
3. **(c)** one audio buffer — voice + noise + hum + 12 clicks — at **312** frames.
4. **(d)** one master audio `.wav`, two scratch `.wav`s derived from it at the authored offsets, and **two VIDEO-ONLY `.mkv` angle sources** (B6).
5. **(e)** reuses (a)'s buffers and its video-only picture verbatim, with every clip cut to 200 frames (B2); the fixture asserts the asset byte identity. `au6_muxed_source()` is built once for rule 7 / E16 and placed in **no** document.

**No fixture bytes are checked in** (`docs/MEDIA-POLICY.md`); the only checked-in AU6 artefacts are `au6_manifest.json` and the three `benchmarks/auto-edit/v7/` files.

### 11.2 Required fixtures

Every entry names its **manifest key** and its **failing direction**, and **every test name is written out** (S9), because the inventory arrays are pinned `[&str; N]` compared as sorted-set equality in both directions.

**Core — `crates/kinewright-core/tests/au6_core.rs` (`AU6_CORE_TESTS`)**

1. **`au6_scenario_geometry_is_the_contract_table`** — key `geometry`. §2.3's fps, raster, rate, channels, lengths, the eight ranges, `AU6_SAMPLES_PER_FRAME == 1_920`, 200 ms = 5 frames. *Fails:* at 30 fps a frame is 1 600 samples and a 200 ms window is not a whole number of frames, asserted.
2. **`au6_every_measured_window_clears_one_gating_block`** — key `geometry.gating`. ≥ 10 frames at 25 fps, with the 2.5× / 5.0× margins recorded. *Fails:* a 5-frame window is asserted under the block.
3. **`au6_the_duck_gap_leaves_exactly_one_unducked_window`** — key `geometry.duck`. A16's arithmetic: hold 200 + release 400 at the head and the next attack 150 at the tail park the bed over 600–850 ms, so exactly **one** aligned 200 ms window — **index 4** — is wholly parked in a 1 s gap and **none** in a 500 ms gap. *Fails:* the 500 ms case is asserted to leave no whole window (N4 S2; never "8 frames").
4. **`au6_the_learn_gap_clears_the_profile_minimum`** — key `learn`. `AU6_C_LEARN_PROJECT_RANGE`'s length ≥ `AU6_LEARN_MINIMUM_PROJECT_FRAMES = 12`, and 12 is re-derived from `ceil(22 528 · 25 / 48 000)`. *Fails:* an 11-frame gap.
5. **`au6_canonical_operations_are_accepted_by_core_in_order`** — key `canonical_documents`. Each batch through `apply_batch` one operation at a time with `validate()` after each. *Fails:* **`au6_ascending_splits_are_rejected_by_core`** — (c)'s and (d)'s splits in ascending order are rejected with `SplitOutsideClip` (A10).
6. **`au6_core_allocates_the_pinned_gap_clip_ids`** — key `canonical_documents.clip_ids`. `AU6_C_RIGHT_CLIP_ID` and `AU6_C_MIDDLE_CLIP_ID` are what core allocates from the base document's next free id (S17). *Fails:* a different base id shifts them.
7. **`au6_the_delivery_scenario_reuses_the_interview_document`** — key `canonical_documents.delivery`. (e)'s operations equal (a)'s except operation 5, whose curve is `AU6_E_DUCK_KEYFRAMES` (the ten `at < 200` keys of `AU6_A_DUCK_KEYFRAMES`); (e)'s document equals (a)'s truncated — every clip `0..200`, `duration 200`, the same assets (B2). *Fails:* a divergent (e) batch, and a curve carrying a key at or beyond frame 200.
8. **`au6_export_jobs_differ_only_in_the_job`** — key `export_jobs`. §2.7's two rows; on the **un-overridden** `profile.export_settings(document, Eight, default())` structs a **field-by-field** comparison **excluding `cancellation`**, difference set `resolution`, `video_bitrate`, `audio_bitrate`, `loudness_normalization`; `delivery_color` asserted **identical**; both targets' `loudness_range_max_lu_hundredths` `None`; and, separately, `au6_export_settings(job, document).resolution == document.resolution` for both jobs (S1). *Fails:* a whole-struct `assert_eq!` is asserted **never** to hold, because `ExportCancellation`'s `PartialEq` is `Arc::ptr_eq` (`crates/kinewright-core/src/media.rs:1402-1406`).
9. **`au6_budgets_are_distinct_from_every_neighbouring_constant`** — key `thresholds.distinctness`. Including `!= 100` against both targets' tolerance. *Fails:* a deliberately equal pair.
10. **`au6_every_budget_carries_the_declared_margin`** — key `budgets`. `AU6_BUDGETS`' 22 rows by `Au6BudgetKind`, plus `AU6_SOURCE_BUDGETS`' three (S7). *Fails:* a floor whose measurement equals its budget.
11. **`au6_the_questions_are_one_clause_each`** — key `review.questions`. One `?`, no `" and "`, and no leak needle in any entry. *Fails:* the brief's original compound questions, quoted in the test.
12. **`au6_the_regression_pins_are_labelled_as_pins`** — key `pins`. The three §11.0.1 pins each carry a doc comment containing the words "regression pin" and the run they were transcribed from. *Fails:* a pin whose doc comment claims a derivation.

**Media — `au6_fixtures.rs` and `au6_sources.rs` (`AU6_MEDIA_TESTS`)**

13. **`au6_every_authored_level_matches_its_analytic_derivation`** — key `sources.levels`.
14. **`au6_the_two_voices_occupy_disjoint_bands`** — key `sources.voices`.
15. **`au6_c_every_authored_gap_is_below_the_silence_threshold`** — key `sources.gaps`, margin printed. *Fails:* **`au6_c_the_brief_levels_never_reach_the_detector`**.
16. **`au6_the_learn_gap_is_the_longest_detected_silence`** — key `sources.learn_gap`. *Fails:* **`au6_c_a_click_in_the_learn_gap_splits_it`**.
17. **`au6_the_scratch_track_is_not_the_master`** — key `sources.scratch`.
18. **`au6_wav_round_trip_is_sample_exact`** — key `sources.lossless`. *Fails:* **`au6_an_aac_mux_is_not_sample_exact`**.
19. **`au6_the_source_shapes_are_the_contract_table`** — key `sources.shapes`. §3.2 rule 7. *Fails:* **`au6_an_angle_source_with_audio_probes_as_audiovideo`**.
19b. **`au6_c_the_degraded_track_is_the_click_free_track_plus_the_clicks`** — key `sources.click_free`. §3.2 rule 8 (A21).
20. **`au6_restated_constants_agree_with_their_owners`** — key `transcriptions`. *Fails:* a deliberately stale copy.
21. **`au6_a_the_interview_clears_the_bed_and_matches_the_voices`** — key `budgets.interview`. *Fails:* 22, 23, 24.
22. **`au6_a_the_unducked_document_does_not_clear_the_bed`** — key `budgets.interview.failing_direction`.
23. **`au6_a_the_untrimmed_voices_are_not_matched`** — key `budgets.interview.failing_direction`.
24. **`au6_a_the_nominal_dbfs_trim_is_worse_than_none`** — key `budgets.interview.wrong_model`. The `+60` trim measures **235** against the un-trimmed **365** and the derived `+37`'s **5** (A3/E3).
25. **`au6_a_the_duck_reaches_its_depth`** — key `budgets.duck`. *Fails:* **`au6_a_the_unducked_bed_has_no_depth`**.
26. **`au6_a_the_duck_depth_is_invisible_at_the_track_point`** — key `budgets.duck.instrument`. `Track(A3)` reads **−1** for the bus-curve owner while `Bus(Music)` reads the real depth, so the measurement point is proved load-bearing.
27. **`au6_the_three_curve_owners_render_identically`** — key `parity.curve_owners`. `assert_eq!` on the master and bus-stem `f32` vectors for all three owners; the track stem asserted to differ for the bus route only (A13).
28. **`au6_every_scenario_mix_does_not_clip`** — key `budgets.clipping`, run on all five scenarios (nit N10). *Fails:* **`au6_a_a_hot_master_clips_and_says_so`**.
29. **`au6_b_the_chain_matches_the_voices_and_reduces_the_spread`** — key `budgets.podcast`. *Fails:* **`au6_b_the_raw_trims_do_not_match_the_voices`**, **`au6_b_the_bypassed_chain_leaves_the_spread_intact`**.
30. **`au6_b_the_programme_stays_inside_its_loudness_range`** — key `budgets.podcast.lra`, via `audio_qc`. *Fails:* **`au6_b_the_makeup_less_chain_exceeds_the_loudness_range`**. **Cuttable.**
31. **`au6_c_the_repair_chain_moves_the_snr_and_the_hum`** — key `budgets.repair`, incl. the per-harmonic selectivity terms. *Fails:* **`au6_c_an_unlearned_profile_moves_no_snr`**, **`au6_c_a_chain_without_the_hum_node_leaves_the_mains_alone`**.
32. **`au6_c_the_declick_node_alone_drops_the_click_error`** — key `budgets.declick`. The fourth document (`au6_c_declick_only_operations()`) through `mix_audio_stems` against `au6_c_click_free_track()`, AU5's line printed, the floor **120** against **258**, and the stem asserted bit-identical to `process_buffer_static` (A21). *Fails:* **`au6_c_a_chain_without_the_declick_node_keeps_every_click`** — 12 clicks and a 0.0 drop.
32b. **`au6_c_the_declick_contribution_in_chain_is_recorded`** — key `declick.contribution_in_chain`. The canonical chain's whole / ±10 ms / ±1 ms de-click contribution against clean (**0.5 / 17.2 / 71.6**), **evidence, not a gate**; the fixture asserts the three are printed and recorded, never that they clear anything.
33. **`au6_c_the_dialogue_survives_the_repair`** — key `budgets.speech_loss`. *Fails:* **`au6_c_an_over_reduced_profile_eats_the_dialogue`**.
34. **`au6_c_the_room_tone_fill_closes_the_gap_seamlessly`** — key `budgets.seam`, a `MeasuredZero` claim through `assert_au6_seam`, plus `track_gaps` empty. *Fails:* **`au6_c_a_one_frame_slip_breaks_the_seam`**.
35. **`au6_c_the_learned_profile_holds_its_pin_and_its_bound`** — key `budgets.profile`. Exact `assert_eq!` against the pin, worst band printed; the one-sided Hann-main-lobe bound at allowance 60; the planner's array asserted equal to `mix_noise_profile`'s. *Fails:* **`au6_c_a_profile_learned_over_speech_is_not_the_noise_profile`** and **`au6_c_a_one_frame_learn_offset_moves_the_profile`** (29 tenth dB at band 0).
36. **`au6_c_the_learned_profile_has_the_authored_shape`** — key `profile.shape`. §2.5's three shape claims. *Fails:* a flat profile is asserted to have no hum bump.
37. **`au6_c_the_percentile_floor_lands_on_the_authored_floor`** — key `profile.estimator`. Measured floor **−4 071** against authored **−4 026**, within **0.45 dB**, recorded as evidence with the gap-fraction reason.
38. **`au6_d_the_master_stem_is_bit_identical_across_the_cuts`** — key `budgets.master_stem`. *Fails:* **`au6_d_a_cut_master_track_is_not_continuous`**, which diverges at sample **576 000**.
38b. **`au6_d_an_angle_ripple_leaves_the_master_stem_alone`** — key `multicam.angle_ripple`. The measured negative (A18): an angle-track `RippleDeleteClip` leaves the master stem bit-identical, because the master clip starts at frame 0. **Evidence, not a gate.**
39. **`au6_d_the_scratch_tracks_contribute_nothing`** — key `budgets.scratch`. *Fails:* **`au6_d_an_unmuted_scratch_track_reaches_the_mix`**.
40. **`au6_d_the_mix_is_the_master`** — key `budgets.master_passthrough`. *Fails:* **`au6_d_the_unmuted_scratch_moves_the_master`** at **600**, 12× the budget.
41. **`au6_d_the_cuts_sit_at_the_authored_frames`** — key `multicam.cuts`. *Fails:* an off-by-one cut frame.
42. **`au6_e_both_deliveries_land_on_their_targets`** — key `budgets.delivery`, both margins printed. *Fails:* **`au6_e_an_unnormalized_export_misses_the_target`**.
43. **`au6_e_the_two_deliveries_separate_by_the_target_difference`** — key `budgets.separation`. *Fails:* **`au6_e_two_exports_at_one_profile_do_not_separate`**. **Cuttable.**
43b. **`au6_e_the_delivery_settings_carry_the_profile_raster_without_rendering_it`** — key `delivery.raster_invariance`. The `resolution` field on `ExportSettings` is the **profile's** (one of B10's four difference-set fields) while the render is overridden to `document.resolution`; the probe-2 raster-invariance table is recorded beside it. **Evidence, not a gate** (A17).
44. **`au6_the_performance_block_matches_its_code_constants`** — key `performance` (nit N13). `AU6_MEDIA_LANE_BUDGET_SECONDS` and `AU6_AGENT_LANE_BUDGET_SECONDS` are asserted equal to the manifest's two `budget_seconds`.

**Agent — `crates/kinewright-agent/tests/mcp_server.rs` (`AU6_AGENT_TESTS`)**

45. **`au6_a1_the_interview_ducks_the_bed_and_matches_the_voices`**
46. **`au6_a2_the_podcast_chain_matches_the_voices_and_tames_the_ride`**
47. **`au6_a3_the_location_dialogue_is_repaired_and_its_gap_filled`**
48. **`au6_a4_the_multicam_cuts_leave_the_master_audio_untouched`**
49. **`au6_a5a_the_delivery_lands_on_the_ebu_r128_target`**
50. **`au6_a5b_the_streaming_target_is_reachable_by_the_agent`** — queue, list, cancel; no encode (B3).
51. **`au6_b_a_clip_whose_head_window_is_silent_gets_no_fade`** — §4(b)(5)'s failing direction.
52. **`au6_the_four_planners_prose_is_pinned_by_exact_string`** — A15's pin on the shipped wordings: three refusals and `plan_clip_fades`' "nothing to propose" prose (§5.5).
    Plus the extended `cc7_the_agent_surface_is_unchanged_by_this_slice` (§5.4).

**App — inline `#[cfg(test)]` (`AU6_APP_TESTS`)**

53. **`au6_a_a_person_can_balance_the_interview_and_ride_the_bed`**
54. **`au6_b_a_person_can_match_the_voices_and_build_the_chain`**
55. **`au6_c_a_person_can_repair_the_dialogue_and_fill_the_gap`**
56. **`au6_d_a_person_can_cut_the_angles_and_mute_the_scratch_tracks`**
57. **`au6_e_the_export_dialog_carries_both_delivery_targets`**
58. **`au6_a_track_chain_offers_both_automation_targets`** — §6.2, incl. the `Fader ↔ "gain_tenth_db"` / `TrackParameter ↔ "pan_percent"` mapping and `chain()` returning `None` for a track.
59. **`au6_a_track_curve_edit_emits_set_track_automation`** — under `track_automation_coalesce_key`, with a scalar drag asserted to coalesce under `track_mix_coalesce_key` instead.
60. **`au6_the_retired_tooltip_no_longer_points_at_a_dead_end`**
61. **`au6_a_track_curve_covers_its_keyframe_span`** — §6.3.
62. **`au6_a_scalar_mix_edit_covers_the_programme`** — §6.3.
63. **`au6_a_cleared_curve_covers_the_span_it_held`** — §6.3.
64. **`au6_the_person_batch_one_code_off_is_not_canonical`** — the shared both-directions helper, exercised per scenario.
65. **`au6_ascending_splits_are_refused_by_the_builder_path`** — the app twin of item 5's failing direction.
66. **`au6_d_the_person_ripple_gesture_is_not_the_canonical_cut`** — the person's blade emits `SplitClip` + `DeleteClip`, and a `RippleDeleteClip` batch is asserted to produce a different document (A18's boundary, on the person side).

**Eval — `crates/kinewright-agent/src/eval.rs` and `src/bin/kinewright-eval.rs` (`AU6_EVAL_TESTS`)**

67. **`published_v7_manifest_tracks_the_audio_workflow_suite`**
68. **`au6_audio_workflow_suite_is_a_packaged_benchmark`**
69. **`au6_audio_evidence_is_computed_where_the_analysis_is_alive`**
70. **`au6_a_delivery_verification_without_a_deliverable_is_recorded_as_an_error`**
71. **`au6_only_audio_assertions_emit_audio_measurements`** — incl. the 63-variant disjointness claim.
72. **`au6_every_audio_assertion_threshold_is_an_au6_scenarios_constant`**
73. **`au6_v6_deliverables_are_byte_identical_without_normalization`**
74. **`au6_the_eval_server_carries_an_exporter`**
75. **`au6_the_blind_package_discloses_no_machine_provenance`**
76. **`au6_accepted_requires_every_audio_question_answered`**
77. **`au6_leak_needles_never_appear_in_a_question`**
78. **`au6_the_audio_questions_are_keyed_by_base_task_id`** — nit N9's claim about `HumanQuestion.id`.

**Inventory (`AU6_INVENTORY_TESTS`)**

79. **`au6_manifest_declares_every_required_fixture_and_constant`**
80. **`au6_declared_test_names_exist_in_their_source_files`**

**No tolerance may be used to excuse a fabricated measurement, a gap that never reaches the silence detector, a profile compared against itself, a regression pin presented as a derivation, a scenario whose two paths were compared against different documents, a blind package that leaks its provenance, a check with no failing case, or a budget no measurement approaches.**

### 11.3 `au6_manifest.json`

`crates/kinewright-media/tests/fixtures/au6_manifest.json`, `include_str!`'d and asserted key by key. **Authored after §12's measurement step, with no unresolved placeholder**, and the **key count asserted**. The audio programme's **first** fixture manifest.

`contract`, `contract_token`, `manifest_version: 1`, then `scenarios`, `geometry` (incl. the duck arithmetic and the gating-block margins), `sources` (per buffer: role, authored level, measured level, frames, `sha256`; for the muxed source and the angle sources the **exact ffmpeg argument vectors** of §3.3), `learn`, `profile` (the 31 pinned values, the analytic mean-power array, the drift and leakage budgets, the worst band), **`declick`** (`click_shape: { samples: 2, amplitude_hundredths: 50 }`, the row-14 line, and `contribution_in_chain: { whole: 0.5, pm10ms: 17.2, pm1ms: 71.6 }` as evidence, A21), `thresholds` (one key per §2.8 constant, asserted equal to the code constant, key count asserted), `budgets` (per row: `{ budget, measured, margin, kind }`, plus `failing_direction` mirroring §4.2 and `reported_not_gated`), `export_jobs`, `canonical_documents` (items 5–7), `pins` (12), `transcriptions` (20), `parity` (27), `multicam` (38b, 41), `delivery` (43b, and the **only** home of the raster-invariance table), **`performance`**, `eval`, `review`, `scorecard`, `m36` (`138 / 1540264 / 1397156 / 120458 / 7 / 5660`, `changed_by_au6: false`), `external_owners`, `required_fixtures`, `manifest_self_test` — **twenty-four** top-level keys, the count asserted (N4 S8).

```json
"performance": {
  "media_lane": { "measured_seconds": 90, "budget_seconds": 180, "engine_constructions": 1,
                  "encodes": 2, "encode_programme_seconds": 8,
                  "breakdown": { "interview": 20, "podcast": 18, "repair": 11,
                                 "multicam_mix": 9, "delivery": 30, "stem_pins": 2 } },
  "agent_lane": { "measured_seconds": 60, "budget_seconds": 120,
                  "tests_using_start_with_exporter": ["au6_a5a", "au6_a5b"],
                  "encodes": 1, "encode_programme_seconds": 12,
                  "breakdown": { "au6_a1": 5, "au6_a2": 9, "au6_a3": 15,
                                 "au6_a4": 5, "au6_a5a": 24, "au6_a5b": 2 } },
  "measurement": { "os": "…", "kernel": "…", "toolchain": "…", "ffmpeg": "…", "commit": "11a6098",
                   "profile": "debug" }
}
```

**Both budgets are recorded, never gated** — `ci.yml` has no `timeout-minutes`, so the failure mode is slow rather than red — but item 44 asserts each equals its code constant (nit N13). The agent breakdown sums to **60** (N4 N14): `au6_a5a` is probe-1 §5's 12 s row (21 005 + 1 419 ms) plus the ≈ 1.2 s of per-test overhead the probe's 16 s estimate carried beyond its 8 s encode, and `au6_a5b` is that overhead alone, rounded up — a queue and a cancel, no encode (B3). The raster-invariance table is **not** here: it lives under `delivery.raster_invariance` (item 43b, S8).

### 11.4 The inventory and the `uses_outside_prose` guard

```text
AU6_MEDIA_TESTS   AU6_CORE_TESTS   AU6_AGENT_TESTS   AU6_APP_TESTS   AU6_EVAL_TESTS
AU6_INVENTORY_TESTS   AU6_EVIDENCE_FIXTURES   AU6_EXTERNAL_OWNERS
AU6_TEST_SOURCES: [(&str, &str); 11]
AU6_EXPLICIT_TEST_NAMES: [&str; 1] = ["published_v7_manifest_tracks_the_audio_workflow_suite"]
```

**`AU6_EXPLICIT_TEST_NAMES` exists because the reverse scan is by prefix** (N4 S14): every declared name must start with `au6_` **or** be named here, exactly as CC7's `CC7_EXPLICIT_TEST_NAMES: [&str; 1]` names `published_v6_…` (`crates/kinewright-media/src/cc7_fixtures.rs:4256-4260`); without it the both-direction assertion could never find item 67.

Each array is compared as **sorted-set equality in both directions**. `AU6_TEST_SOURCES`' **eleven** entries (B3): `au6_fixtures.rs`, `au6_sources.rs`, `../../kinewright-core/tests/au6_core.rs`, `../../kinewright-agent/tests/mcp_server.rs`, `../../kinewright-agent/src/eval.rs`, `../../kinewright-agent/src/bin/kinewright-eval.rs`, `../../kinewright-app/src/mixer_ui.rs`, `../../kinewright-app/src/mixer_pane_ui.rs`, `../../kinewright-app/src/edit_diff.rs`, **`../../kinewright-app/src/inspector_ui.rs`**, **`../../kinewright-app/src/app.rs`** — the last two because §0.2 item 18 promotes `clip_audio_operation` and `noise_profile_operation` to `pub(crate)` there. `include_str!` makes this a **compile-time** dependency, which is why the inventory is §12's last step. `declares_test` requires a real `#[test]` / `#[tokio::test]` attribute above `fn name(`.

**The guard.** `uses_outside_prose(source, needle)` counts a needle used when a non-comment line contains `needle(` or `("needle")`, so the needles **must** be an array literal — a single-argument helper call self-matches, and CC6/CC7 escape only because theirs sit in one:

```rust
const AU6_FORBIDDEN_HELPERS: [&str; 3] = [
    "fixture_gpu_or_skip",
    "KINEWRIGHT_GPU_TESTS_MAY_SKIP",
    "KINEWRIGHT_AUDIO_TEST",
];
```

**`cfg(target_os` is asserted separately with a plain `!source.contains("cfg(target_os")`** (S10): a real occurrence is `#[cfg(target_os = "windows")]`, which contains neither `cfg(target_os(` nor `("cfg(target_os")`, so a fourth needle in the array would be **inert**.

**AU6 records a CC7 erratum here:** `CC7_FORBIDDEN_HELPERS` is `[&str; 3]` in code (`crates/kinewright-media/src/cc7_fixtures.rs:4298-4302`, the third being `"std::env::var"`) while CC7 §11.3 states `[&str; 2]` (§12.1).

---

## 12. Implementation order

**Size, estimated.** CC7 estimated 13 000–18 000 insertions for six scenarios in five crates (`docs/CC7-WORKFLOW-EVALUATION.md:1309`). AU6 writes five scenarios, one product change and a smaller harness delta, but adds an audio-carrying source recipe and the audio programme's first inventory. Estimate **11 000–15 000 insertions**, split roughly: `au6_scenarios.rs` 1 300, `tests/au6_core.rs` 1 100, `au6_sources.rs` 900, `src/au6_fixtures.rs` 3 000, `au6_manifest.json` 900, `tests/mcp_server.rs` +1 500, app +1 300 (the §6.2 change at 300–450, §6.3 at ≈ 40, the rest tests), eval (`eval.rs` + `kinewright-eval.rs`) +3 200 (the evidence block at 400–600, six variants, the suite, the blind arms, the unit tests), `benchmarks/auto-edit/v7/` 400, docs 700. The measured number replaces this estimate in §0.3.

**Two parts, one contract.** Part A lands as **`feat: complete AU6a audio workflow gates`** (§2–§6 and §11); Part B as **`feat: complete AU6b audio workflow evaluation`** (§7, §8 and §12.1). Inside each part the order is the audio programme's recipe (N0, N1 nits 10–12): **core first, then media, then agent and app in parallel by disjoint files, then the `include_str!` inventory last** — last because it is a **compile-time** dependency on every other crate's test sources (§11.4). Each part ends with **two review passes per crate it touches**, then the workspace gate — `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace` — clean before its commit; every cargo command is prefixed `source scripts/setup-ffmpeg.sh >/dev/null 2>&1;`; the gate is run when no other cargo test is live (§14); push only on Riel's go. A step that finds this text wrong records a §0.3 erratum in its crate's reserved range and does not edit another crate's files.

**Cut order.** Exactly two fixtures are marked **cuttable** in the body, and each is cuttable because its *failing direction* costs a render the canonical measurement does not:

1. **§4(e)(3), item 43 — `au6_e_the_two_deliveries_separate_by_the_target_difference`'s failing direction.** The separation itself is arithmetic on the two verification reports (e)(1) already produces and costs nothing; `au6_e_two_exports_at_one_profile_do_not_separate` is a **third 8 s encode**, ≈ 15 s of the media lane. Fallback: the row stays in `AU6_SOURCE_BUDGETS` as `RecordedMargin`, the third encode is dropped, and §13 records the cut with the date.
2. **§4(b)(3), item 30 — `au6_b_the_programme_stays_inside_its_loudness_range`'s failing direction.** The LRA comes from the `audio_qc` call §4(b)(4) already makes; `au6_b_the_makeup_less_chain_exceeds_the_loudness_range` is a fourth (b) document rendered through the chain, ≈ 6 s. Fallback: as above, with probe-1's measured **1 274** kept in the manifest as `reported_not_gated`.

**Never cut**, because they are the exit gate: the five sources and their eight non-vacuity fixtures (§3.2); the six agent end-to-end tests (§5.2); the five person-path tests **and the §6.2 change** — if §6.2 slips it is not a cut but the checked fallback sentence of §6.6; §4(d)(1)'s master-stem identity, which is the gate that replaced (d)'s delivery leg (A1); §4(e)(1) and (e)(2) on **both** lanes; the blind package and its leak test (§8.5); and **the audio evidence block** (§7.6) — CC7's first cut is not available to AU6, because six of the seven `EvalAssertion` variants are unreachable without it.

### 12.1 Documentation deliverables, and the errata AU6 discharges

Part B's step 12. Every item is a file edit, named:

1. **This file**, promoted to `docs/AU6-WORKFLOW-EVALUATION.md` with §0.3 filled from both parts' errata and the measured insertion count.
2. **`docs/ROADMAP-AND-WORKFLOWS.md`** — the status paragraph at `:663-690` is stale ("AU5 lands in two parts"; N0.3(e)) and is rewritten: AU5 landed (`4f25cba`, `039cde4`); AU6 complete in two parts on one contract; **the audio programme table AU1–AU6 is complete**; the hands-on sessions still owed are listed by name — **AU1**'s platform smoke, **AU2 Part B**'s smoke (which the paragraph omits), **AU3 Part B**'s export, **AU4 Part B**'s ride, **AU5 Part B**'s listening session — plus AU6's own two (§13). The AU6 row at `:661` is unchanged; the next-slice line says the audio programme is complete and names what §13 leaves open.
3. **`CHANGELOG.md`** under `[Unreleased]` — one entry per part.
4. **`docs/M36-AGENT-RUNTIME-EFFICIENCY.md`** — two rows appended after the AU5 Part B rows (`:124-132`): "Served MCP runtime (after AU6 Part A)" and "(after AU6 Part B)", each **7 / 5 660 B / 3 510 B / 998 B**, with one sentence each stating the registry is unchanged (§5.4), CC7's rows there at `:137`.
5. **`docs/EVALS.md`** — the run line and usage string for `audio-workflow-v7`, and a `### audio-workflow-v7` section after `### color-workflow-v6` (`:367-375`) in its words: *pending real-harness run*, CI covers the unit tests and spends nothing, no model result is recorded. It also states the gap it inherits: **there is no published v6 result** either (N0.4(a)), and AU6 does not pretend a v7 run exists.
6. **`benchmarks/auto-edit/v7/README.md`** (§7.9) — v6's structure, the listening-level request of §8.0, the `proof.png` sentence of §7.2.
7. **`README.md:42-43`** — the benchmark links run v1–v5 and omit v6; the line gains v6 and v7.
8. **CC7 §0.3 gains three one-line errata, `H-E1`–`H-E3`.** `H` is free: §0.3's letters are A–G and X–Z. **H-E1:** `CC7_FORBIDDEN_HELPERS` is `[&str; 3]` in code (`crates/kinewright-media/src/cc7_fixtures.rs:4298-4302`, the third needle `"std::env::var"`); G-E4 (`CC7:162`) records the change but §11.3's normative sentence (`CC7:1304`) still reads `[&str; 2]` and is read through G-E4. **H-E2:** G-E6 (`CC7:164`) and `CC7:219` say **95** declared tests; `cc7_fixtures.rs:4563` asserts **96**. **H-E3:** `CC7:853` cites `app.rs:180` / `:1858`; the fn is `crates/kinewright-app/src/app.rs:206` and its call site `:2175` (§6). AU6 re-baselines nothing in CC7; the three lines are pointers.
9. **The debts AU5 §0 left owed (N0.3(e)).** *(a)* The AU4 §6.2 pointer erratum AU5 R80 says it could not write (`docs/AU5-REPAIR-AND-ROOM-TONE.md:601-605`) **already exists**: AU4 §0 **E59** (`docs/AU4-CLIP-ENVELOPES-AND-AUTOMATION.md:486-492`, dated 2026-09-10) is that pointer. AU6 closes R80's "Owed" sentence with one parenthetical naming E59 rather than writing a second erratum. *(b)* AU5 R111's owed words (`AU5:1307-1308`): AU5 §8's Part B media file list gains `audio.rs`, and media `lib.rs`'s `au5b_fixtures` doc comment (`crates/kinewright-media/src/lib.rs:59-63`) — which says `mix_audio` is `pub(crate)` "as is `test_support`" while `test_support` is a `pub mod` — loses the clause; implementer K's file, one line. *(c)* The slice-list comment at `crates/kinewright-agent/src/server.rs:26649-26651` stops at "AU5 §4.3/A18" and gains "AU6 §5.4 Part A and Part B" — implementer L, one comment line, beside the two ledger paragraphs of §5.4 in `tests/mcp_server.rs`.
10. **`docs/DESIGN.md`** is **not** here: its two edits are Part A's step 7 (§6.2 step 6), because the pin test that reads it lands in the same step.

### 12.2 Part A — `feat: complete AU6a audio workflow gates`

1. **Core authority (J).** `crates/kinewright-core/src/au6_scenarios.rs` in full (§2): the five `Au6ScenarioSpec`s, the turn table with (c)'s eighth-gap override, the pinned plan arguments, the **three regression pins with their doc comments** (§11.0.1), `au6_canonical_operations` and the six operation builders including `au6_c_gap_operations(next_clip_id)` and `au6_c_declick_only_operations()` (A21), `AU6_EXPORT_JOBS` and `au6_export_settings` with the raster override (§2.7), `au6_c_analytic_mean_band_tenth_db`, `AU6_BUDGETS: [Au6Budget; 22]`, `AU6_SOURCE_BUDGETS: [Au6Budget; 3]` (S7), `AU6_E_DUCK_KEYFRAMES` (B2), `Au6TrackRole::Picture` (S12), `AU6_QUESTIONS`; `lib.rs` registration and re-export; `crates/kinewright-core/tests/au6_core.rs` items 1–12. **Every row holds a measured number; no placeholder survives probe-3 (§0.2 item 50).** *Size ≈ 2 400.*
2. **Media generators (K).** `crates/kinewright-media/src/au6_sources.rs` (§3) under the existing test-util gate, the module doc in `cc7_sources`' words, `au6_stamp_on_project_grid` (A11), the `RawFrames`-shaped `Drop` guards on both temp files, the §3.3 mux recipe; the eight non-vacuity fixtures of §3.2 (items 13–19b) in `src/au6_fixtures.rs` around the one shared `FfmpegMediaEngine`. *Size ≈ 1 600.*
3. **Media gates (K).** `src/au6_fixtures.rs` items 20–44: the two margin helpers (S16), `assert_au6_seam`, every §4 gate written against §2.8's constants with the failing directions of §4.2, each margin printed in AU5's line shape. *Size ≈ 3 000.*
4. **Measurement and paste — the hard barrier.** *(i)* Every constant is already in the text — probe-2's seven values (§0.2 items 46–49) and probe-3's de-click floor (item 50) — and this step **confirms** them; probe-3's throwaway module (`target/review/au6/probe/probe3-media-src-export.rs`) is the implementer's reference for the error-drop arithmetic, and its numbers are pasted, **never re-derived**. *(ii)* The media lane runs **once** on the implemented fixtures, and every §4.1 row must reproduce its measured column inside its printed margin. A row that moves is a §0.3 erratum recording **both** numbers; a budget is never widened. *(iii)* The media `performance` lane is recorded from this run; the agent lane from step 6's first green run; both are regenerated once more at step 8. **Steps 5–8 must not assert a budget before this step lands.**
5. **Fixtures and manifest (K).** `crates/kinewright-media/tests/fixtures/au6_manifest.json` authored from step 4's numbers (§11.3), **no unresolved placeholder**, the key count asserted; the constant half of item 79 — every top-level key of §11.3, `performance` included — and item 44. **The inventory arrays and the declared-name half do NOT land here** (step 8). *Size ≈ 900.*
6. **Agent scripts (L).** The six `au6_` tests of §5.2 with §5.3's four advances and two `SilenceStatus::Ready` waits, items 51–52 (the four prose constants of §5.5), the pid + thread temp path, `start_with_exporter` for `a5a` (the 12 s encode) and `a5b` (queue, list, cancel — B3) and `start_isolated_with_exporter_and_project_path` for `a3`; the two ledger paragraphs appended **after the AU5 Part B paragraph** and the four extended assertion messages of `cc7_the_agent_surface_is_unchanged_by_this_slice` (§5.4); the one comment line of §12.1(9c). *Size ≈ 1 500.* **Parallel with step 7.**
7. **Person path and the two product changes (M).** In this order inside the step: *(a)* the two `pub(crate)` promotions (`crates/kinewright-app/src/inspector_ui.rs:4503`, `crates/kinewright-app/src/app.rs:2647`), recorded as an R60-range erratum; *(b)* §6.2's six numbered edits — `MixerSelection::Track(TrackId)` with `chain(self) -> Option<AudioChain>` and its four call sites, `MixerChain::Track`, `AutomationTarget::TrackParameter(&'static str)` with the two-way mapping and its three arms, `MixerChainEdits.tracks` and `drain_into` emitting `SetTrackAutomation` under `track_automation_coalesce_key(track, parameter)` and `SetTrackMix` under `track_mix_coalesce_key(track)`, `edit_toggle` on the track strip, the retired `AUTOMATED_FADER_TOOLTIP` wording, and the **two** `docs/DESIGN.md` edits including "a track, bus or master ride" — **≤ ~450 app lines**; *(c)* §6.3's `changed_project_range` rule and `clip_content_equal`'s `audio_gain_curve`, ≈ 40 lines in `crates/kinewright-app/src/edit_diff.rs`; *(d)* the five scenario tests and items 58–66 in `mixer_ui.rs`'s and `edit_diff.rs`'s `mod tests`. If *(b)* slips, `au6_spec(Interview).person_path` becomes `NotApplicable` and §6.6's fallback sentence is written — a checked fact, not a cut. *Size ≈ 1 300.* **Parallel with step 6.**
8. **Performance regeneration and the inventory, last (K).** Re-measure both `performance` lanes on the finished Part A tree in one run and paste them into the manifest; then `AU6_MEDIA_TESTS`, `AU6_CORE_TESTS`, `AU6_AGENT_TESTS`, `AU6_APP_TESTS`, **`AU6_EVAL_TESTS: [&str; 0]`**, `AU6_INVENTORY_TESTS`, `AU6_EVIDENCE_FIXTURES`, `AU6_EXTERNAL_OWNERS`, `AU6_TEST_SOURCES: [(&str, &str); 11]`, `AU6_EXPLICIT_TEST_NAMES: [&str; 0]` (filled at step 13), `AU6_FORBIDDEN_HELPERS`, the plain `cfg(target_os` assertion, and items 79–80 (§11.4). `include_str!` binds this file to step 6's and step 7's sources **at compile time**, which is why nothing here can be authored earlier and why the both-direction assertion would fail on any `au6_` test that exists but is not yet declared. *Size ≈ 400.* Then Part A's two review passes per crate (core, media, agent, app), the workspace gate, and the commit.

### 12.3 Part B — `feat: complete AU6b audio workflow evaluation`

9. **Harness plumbing (P), `crates/kinewright-agent/src/eval.rs` only.** `EvalDeliverableSpec.normalize_to_profile_target` as a **plain field** with the five `eval.rs` literal sites at `false` and `export_and_probe`'s one assignment (§7.7); `McpServer::start_with_exporter` at `:1389-1394` (§7.8); the audio evidence block — `AudioEvalRequest`, `AudioEvalEvidence` keyed by point label, `AudioEvidenceError` / `AudioEvidenceQuantity`, `EvalOutcome.audio`, `EvalDefinition.audio` with the six `eval.rs` test literals, `measure_audio_block` beside `measure_color_block` and **before** `restore_original` (§7.6); the six `EvalAssertion` variants, `is_audio_assertion`, `assertion_measurement` keeping `ungated_color_measurements` (§7.5); `AUDIO_WORKFLOW_BENCHMARK_ID`, `AUDIO_WORKFLOW_NOT_APPLICABLE` and the `human_review_template_with_questions` arm (§8.2); the `HumanQuestion.id` doc-comment edit (`eval.rs:1071`, N13); items 69–74 and 76. **The workspace does not compile between this step and step 10's first act; both land in one commit.** *Size ≈ 1 900, of which the evidence block is 400–600 and its own implementer.*
10. **Suite, review package and blind arms (Q), `crates/kinewright-agent/src/bin/kinewright-eval.rs`.** First act: the seven `normalize_to_profile_target: false` sites (§7.7) and the **twenty** `audio: None` sites (§7.6, S10). Then `audio_workflow_suite` with `#[allow(clippy::too_many_lines)]`, `audio_workflow_budget`, the six fixture builders including `fixture_au6_location_dialogue()`'s `TempDirectory`, the `eval_suite` arm, **`is_packaged_benchmark` — mandatory** (§7.1), `usage_text`'s one literal, `review_questions`' audio arm keyed by base task id, `au6_canonical_parameter_names()`, the leak test and its needles (§8.5); items 68, 75, 77 and 78. *Size ≈ 1 300.*
11. **Published benchmark (Q).** `benchmarks/auto-edit/v7/manifest.json` (twelve top-level keys, six tasks, the six-key `acceptance_target`), `ground-truth.json` generated by `au6_ground_truth_json()` with byte equality asserted, `README.md` in v6's structure (§7.9); item 67, `published_v7_manifest_tracks_the_audio_workflow_suite`, with its `machine_assertions` string check. *Size ≈ 400 plus the generated JSON.*
12. **Docs (§12.1).** *Size ≈ 700.*
13. **Inventory re-opened, last (K, one edit).** `AU6_EVAL_TESTS` from `[&str; 0]` to **`[&str; 12]`** (items 67–78), **`AU6_EXPLICIT_TEST_NAMES: [&str; 1]`** naming item 67 (S14), and the manifest's `required_fixtures` eval entries; `AU6_TEST_SOURCES` is **unchanged**, because `eval.rs` and `kinewright-eval.rs` were already `include_str!`'d in Part A. The both-direction assertion is what forces this edit into the same commit as the tests it names. Then Part B's two review passes (agent; media for the one edit), the workspace gate, and the commit.

### 12.4 Dependency sentence and implementer file sets

Steps 1 → 2 → 3 → 4 → 5 are strictly ordered. Steps 6 and 7 depend on 1 and 2 and may start beside 3, but **neither may assert a budget before step 4 lands**. Step 8 depends on 5, 6 and 7. Part B depends on the Part A commit. Steps 9 → 10 → 11 are strictly ordered; step 12 may run beside 10 and 11; step 13 depends on 10 and 11. The inventory is last in **each** part for the same compile-time reason.

| Implementer | Crate | Files, exhaustively |
| --- | --- | --- |
| **J** | core | `crates/kinewright-core/src/au6_scenarios.rs` (new); `src/lib.rs` (one `pub mod` and one re-export); `tests/au6_core.rs` (new). **No other core file**: `AudioChain`, `MixSpectrumPoint`, `operation.rs` and `model.rs` are untouched (§0.2 items 16, 22). |
| **K** | media | `src/au6_sources.rs` (new); `src/au6_fixtures.rs` (new); `src/lib.rs` (two module lines and §12.1(9b)'s one clause); `tests/fixtures/au6_manifest.json` (new). `export.rs` is **not** edited: `mix_audio_stems` stays `pub(crate)` (A9). |
| **L** | agent | `tests/mcp_server.rs` (six tests, items 51–52, the two ledger paragraphs, the four extended messages); `src/server.rs` — **the one comment line at `:26649-26651` and nothing else**. |
| **M** | app | `src/mixer_ui.rs`, `src/mixer_pane_ui.rs`, `src/edit_diff.rs`; `src/inspector_ui.rs` and `src/app.rs` for the two `pub(crate)` promotions **only**; `docs/DESIGN.md` (§6.2 step 6). **`src/export_ui.rs` is read, not edited** — the (e) test lives in `mixer_ui.rs`'s `mod tests` and calls `export_loudness_target` (`export_ui.rs:759-764`), which is already `pub(crate)` (nit N14: the set is unconditional). |
| **P** | eval harness | `src/eval.rs` only (step 9). |
| **Q** | eval suite | `src/bin/kinewright-eval.rs` (step 10); `benchmarks/auto-edit/v7/` (step 11). |
| orchestrator | docs | §12.1 items 1–9, and the paste of step 4(i). |

Erratum ranges follow the crate, not the implementer (§0.3): P and Q both write **R80–R99**, and a P/Q collision is resolved by the first to commit an erratum number taking it.

---

## 13. Explicit deferrals

Each names why it is a slice and not a flag, and carries the cost the body promised where it promised one.

**Product gaps the evaluation found and did not fix (§1, §6.5, N1 S3(b)).** Only the two §6.2/§6.3 changes met the exception rule (the app's own text directed the person to a dead end, ≤ ~450 app-only lines). The rest:

- **`RemoveAudioBus` and bus rename controls.** Half a day (N1 S3(b)): a destructive strip control needs the confirmation shape the timeline's delete already has, a rename needs an inline text edit the mixer has no precedent for, and both need their DESIGN.md sentence. **Not free:** the person path for every AU6 scenario builds buses and never removes one, so the gap costs no scenario. *With it:* the **AU2 `remove_audio_bus` destructive-annotation asymmetry** — in `plan_preview`'s destructive list (`crates/kinewright-agent/src/runtime.rs:365`) but absent from `destructiveHint` (`crates/kinewright-agent/src/schema.rs:410-413`), pinned by a test (N0.6(i), S12). An hour of code, but a `destructiveHint` flip moves descriptor bytes and therefore the ledger, which AU6 may not touch (§5.4).
- **A `plan_track_mix` planner.** Two days: it needs a bus-role vocabulary the model has none of (N0.8(c)), and A3/E3 shows the obvious algorithm — trim by the nominal dBFS difference — is the **wrong model** by 2.35 LU; a real planner has to meter with `mix_levels` and iterate. **Not free:** (a) and (b) show the person and the scripted agent reach the matched document with two `set_track_mix` calls and the measured trims.
- **A sync-group editor, an angle viewer, audio-follow, a master-audio track type, audio-based sync detection.** Roadmap `:148`'s deferred column. Nothing in the tree derives or verifies an audio offset (N0.7(e)); a clap or cross-correlation detector is a slice with its own fixtures. **Not free:** (d)'s `SyncGroup` is base-document state with caller-supplied integer offsets, and §4(d) proves the cuts honour them, not that they were recovered.
- **`plan_speaker_multicam` and `ThreePointEdit` as (d)'s instruments.** The planner refuses a non-video target track (`crates/kinewright-core/src/multicam.rs:188-189`), emits `Operation::ThreePointEdit` (`:427-439`) rather than the `SplitClip` + `DeleteClip` a person's blade writes, and needs a diarized transcript, which no synthetic fixture can produce because CI has no speech (N0.7(c)). A speaker-driven multicam evaluation is a slice that needs real diarized material and its own ground truth. **Not free:** (d) proves the master-track policy with the operations both paths share, which is the claim the roadmap row makes.
- **An eyedropper beyond `Learn profile`; a loudness-matched A/B hold; an audio QC pane; bus solo/mute; scrub audio.** The A/B hold is a day (a monitor gain stage that is not the master fader) and is why §8.0 asks for one listening level; the QC pane is two days (a surface for `AudioQcReport` the app has no precedent for); bus solo/mute is half a day of app **plus** an `AudioBus` field, i.e. a schema byte, so it is not app-only; scrub audio is engine work. **Not free:** every AU6 scenario completes by hand without them (§6.6).
- **The app/agent bitrate-and-raster divergence.** `KinewrightApp::start_export` hard-codes `video_bitrate: 8_000_000` / `audio_bitrate: 192_000` for every profile (`crates/kinewright-app/src/export_ui.rs:1734-1735`) while the agent's `queue_export` takes the profile's `export_settings`. Half a day, but it changes what the app writes for **every** export and needs its own fixture. **Not free:** §6.4's builder-level identity proves the loudness target is the same on both paths, which is the thing AU6 gates.
- **A live `KinewrightApp` harness.** CC7 §13 unchanged: `KinewrightApp::new` is private (`crates/kinewright-app/src/app.rs:206`, `:2175`), there is no `crates/kinewright-app/tests/`, and a harness needs a constructible app, an injectable media engine, a deterministic frame pump and a way to assert a rendered frame. The §6.2 change is therefore proven at builder level and heard by hand (the checklist below).

**Measurement limits AU6 recorded rather than papered over.**

- **The manufactured 50 Hz `mains_hum_present` finding after a 60 Hz cascade (E7, A7).** `hum_50_excess_db_hundredths` rises from **0 to 829** (planner chain) / **1 072** (media chain) because the 50 Hz estimator's sixth-octave shoulders at `f·2^(±1/6)` (`crates/kinewright-core/src/audio_repair.rs:109-113`) sit where the 60 Hz notch's shoulders land. An AU5 limit: the fix is a shoulder reference that excludes notched neighbours, or a joint two-family report — a day in media plus a report-shape change, and a new fixture with both families present. **Not free:** AU6 gates the 60 Hz drop and the per-harmonic vector, which are the claims the repair actually makes, and asserts nothing about the 50 Hz term.
- **The fourth harmonic.** `REPAIR_HUM_HARMONIC_COUNT = 3` (`crates/kinewright-agent/src/server.rs:10228`) notches 60/120/180 Hz; the report publishes **four** entries and the fixture authors **five** partials, so 240 Hz is un-notched and its excess **rises 103** (A19). The fix is a `harmonic_count` argument on `plan_dialogue_repair` or a raised constant — an AU5 planner amendment whose schema grows, i.e. a ledger byte and the M36 procedure; half a day plus the ledger. **Not free:** AU6 gates the first three and asserts the fourth is **not** claimed.
- **A closed-form derivation of the 20th-percentile profile.** The reduction is a percentile of Hann-windowed periodogram band powers over ten 4 096-frame windows of LCG-uniform noise plus partials, through a half-bin-overlap integrator, `div_euclid(10)` and a clamp (§2.5). A day of derivation with no guarantee of a closed form. **Not free:** the one-sided Hann-main-lobe bound holds for all 31 bands at 60 tenth dB and the exact pin catches any drift; a two-sided gate would add nothing the regression pin does not already prove.
- **AU5 R30, the de-click ceiling on real material — re-deferred to Riel's hands-on session by name.** R30 (`docs/AU5-REPAIR-AND-ROOM-TONE.md:144-147`) expected "AU6's real material" to move the ceiling. AU6 has **no real material in CI and cannot** (N0.7(c): no committed media bytes, no speech synthesis on Linux CI); its clicks are 2-sample ±0.5 impulses under an analytic carrier (A20). Structurally undischargeable here.
- **AU4 rule 24 — no bus or master ripple.** `docs/AU4-CLIP-ENVELOPES-AND-AUTOMATION.md:844-848` says "revisit in AU6". AU6 measured the adjacent fact: an **angle** ripple provably cannot move the master track stem (A18, `au6_d_an_angle_ripple_leaves_the_master_stem_alone`) and `sync_lock` governs participation, not which clips move. The bus/master **curve** question is unchanged and the limit stands; `changed_project_range` (§6.3) treats a ripple as a clip edit, as it already did.
- **Real speech, real music, a real room.** `scorecard.generalization_matrix` publishes them `false` (§8.7). No committed media bytes; espeak-ng is not in `install-linux-deps.sh`, SAPI is Windows-only and both TTS paths are env-gated; v5's real speech is `#[ignore]`d behind pinned downloads. **Not free:** the Schroeder carriers are what makes every level analytic (§3.2).
- **A multicam encode.** (d)'s delivery leg is cut (A1: 21.6 s at 12 s, ≈ 15 s at 8 s). No gate proves a multicam document survives the encoder; restoring it costs that time and one `au6_a4` exporter. **Not free:** the bit-identical master stem is the stronger claim, and (e) proves the encoder on (a)'s document.
- **A recorded cross-platform decoded delta.** CC7 §13 unchanged: it needs an artefact-comparison subcommand, a store for one OS's encode, and a budget that is not an inherited codec tolerance. Windows numbers are recorded, never gated (Appendix A, row W).
- **Intelligibility beyond WER; per-word ASR confidence; a dialogue-versus-music balance tool.** WER against authored truth is the only proxy (N0.8(c)); Whisper's probabilities are discarded; dialogue-over-bed is derivable from `get_audio_levels` and derived nowhere in the product — AU6 derives it in a fixture and in `DialogueOverBedAtLeast`, never in a tool. A balance tool is a planner slice.
- **The LRA floor below 8 s.** Probe-1 §5.1: 8 s is the *shortest length this material was measured at*; a shorter programme eventually loses LRA (EBU 3342 needs at least two gated 3 s short-term windows) and the probe did not find that floor. A later slice that shortens the encode lanes must re-measure.
- **`AudioQcReport.channel_balance_lu_hundredths`** stays unread by any assertion (N0.4(c)): every AU6 buffer is authored mono and interleaved identically, so the term is identically zero and a gate on it could not fail (11.0.5).

**Named follow-ups outside AU6, filed by this slice's review.**

- **One FFmpeg build on both operating systems, or a per-OS measured column and the three contracts that forbid it.** Windows CI provisions `System233/ffmpeg-msvc-prebuilt ffmpeg-8.0.1-r3` and Linux provisions `mifi/ffmpeg-builds 8.0-1`, and with the CI gate finally working the difference is no longer hypothetical: four `kinewright-media` tests fail on Windows purely because the encoder and scaler round differently (§0.3 review finding 5). AU3 §0 E55, CC6 §6.3 and AU6 §10.6 each say a per-OS figure earns a note on the constant's doc comment and **never** a per-OS constant, and §4.1 note 1 forbids inventing a tolerance to absorb one — so no slice can discharge this on its own. The three options are: provision one build on both systems (the cheapest to reason about, and it makes every existing measured column true on both); publish a per-OS measured column and amend those three contracts; or scope the encoded fixtures to Linux and say so in each contract's exit gate. **This is a cross-slice decision and it blocks every Windows measurement**, including AU6's: §13 item 5 and Appendix A row W stay empty until it is made.
- **Whatever the first Windows run that reaches the AU6 lanes finds.** The four pre-existing `kinewright-agent` failures that used to abort `cargo test --workspace` on `windows-latest` are fixed in this branch (§0.3 review finding 2); the seven `kinewright-media` failures now behind them (review finding 5) are next. **Nothing about AU6 on Windows has ever been measured**, and §14 names the exposure: four rows at 2.03–2.22×, an `f64` seam asserted exactly zero and a 31-band exact pin. A row that lands inside its budget but under 2× earns a per-OS note in the constant's doc comment, never a per-OS constant (§10.6); a nonzero band delta on the pin is re-baselined once with both numbers recorded (§0.3's standing rule). Until that run exists this is the largest unknown in the slice.
- **The AU6 lane cost on Linux CI.** AU6 adds ≈ **10.5 minutes** to every Linux run: the `kinewright-media` lib binary goes to **667 s** and the job from 8 m 58 s to 19 m 27 s (R23). Nothing is red — `ci.yml` sets no `timeout-minutes` and both lane budgets are recorded, never gated — but §14's ≈ 150 s projection was out by 7× and the cost is now owned here rather than discovered. The two levers, in order: §12's cut order (two fixtures, ≈ 21 s of encode) buys far too little, so the real one is the **call pattern** — the lane's floor is `mix_levels` at 8 829 ms per whole-programme call and the encodes at 8 s each, and a later slice that wants the lane back under five minutes has to share renders across fixtures, which means a fixture-level cache with its own invalidation rules. A day in `au6_fixtures.rs`, and it trades the one-engine-one-render simplicity that makes the current lane auditable. **Not free:** the alternative is dropping gates, and §12 marks exactly two fixtures cuttable.

**Agent-surface debts, each a ledger byte and therefore not AU6's.**

- **The agent-lane streaming encode at the profile raster.** `queue_export` renders the profile's own raster with no override (`crates/kinewright-agent/src/export_queue.rs:784-793`), so a scripted `youtube_1080p` export of the 12 s document is ≈ 190 s and cannot live in CI; `au6_a5b` queues and cancels (B3). The objective proof of the streaming target is the media lane at the document raster (A17), and the profile-raster encode is proven by the eval lane's `a5b`, which is hand-run (§13 checklist item 3). **Not free:** a raster argument on `queue_export` is a schema byte and a slice.
- **The typed-envelope debt (A15).** The five audio planners return `error_text` prose, take no `expected_revision`, and publish `applied` / `evidence_only` non-uniformly (§5.1); `plan_audio_normalization` refuses a `profile` argument with a schema error; the ten `RoomToneStoreErrorCode` strings reach the agent by string parsing (`crates/kinewright-agent/src/server.rs:13926-13945`) and `room_tone_capture_too_long` has two `details.observed` shapes (§5.5). One typed envelope per planner is a schema change per tool — the served quad and the registry ledger move — so it is a slice under the M36 procedure. AU6 pins the shipped prose by exact string so the debt is visible when it is paid — and records that only **three** planners have a prose *refusal* reachable with valid arguments (`plan_audio_ducking` rule 125, `plan_room_tone_fill`'s missing-asset refusal, `plan_dialogue_repair`'s already-repaired refusal); `plan_clip_fades` never refuses on valid input, it proposes nothing in success prose (N4 S17).
- **`QueueExportArgs` without `deny_unknown_fields`** (`server.rs:11891-11924`, against the AU5 planners' `:11740` / `:11773`). An hour, but `additionalProperties: false` is a schema byte. AU6's cells send only known fields.
- **The duplicated `-6_000..=3_600` normalization gain guard** (`crates/kinewright-media/src/export.rs:181`, `crates/kinewright-agent/src/server.rs:11225-11227`) is unpinned across crates. An hour for a cross-crate test; recorded because AU6 edits neither file.
- **`EvalLoudnessSpec` has no true-peak field** (`crates/kinewright-agent/src/eval.rs:128-132`). AU6 does not extend it: `DeliveryAudioVerified` carries `minimum_true_peak_margin_hundredths` itself, and adding a field to a v5-era spec is a v5 re-baseline.
- **Retrofitting the `machine_assertions` string check to v1–v6** (§7.9; v5 g3's v9/v10 drift, N0.4(d)). Half a day, touching six frozen manifests' tests.

**Review-package limits, unchanged from CC7 §13.** Blinding the scenario identity (§8.0 says what is not blinded); a seventh `HumanRatingDimension` (a wire-schema bump for every historic `human-review.json`); the procedural defeatability of `blind/` (§14).

**The hand-run checklist.** Nothing below is a `cargo test`; each is a person's session, recorded in the roadmap status paragraph until done.

1. **The five sessions Riel already owes** (N0.3(e), roadmap `:663-690`): AU1's platform smoke; **AU2 Part B's smoke**, which the roadmap paragraph omits; AU3 Part B's hands-on export; AU4 Part B's ride (a ride drawn during playback without a re-cue, a bus curve edited in the chain pane, one ducking plan committed and listened to); AU5 Part B's listening session (a profile learned on a noisy location interview and heard, a hum notch checked against its comb, one `plan_dialogue_repair` committed and listened to) — **and inside it AU5 R30's de-click ceiling on real clicks**, above.
2. **AU6's delivery at the profile's own raster (A17).** `e_streaming` encoded once **without** the §2.7 override — `settings.resolution` left at `Youtube1080p`'s 1920×1080 — through `queue_export` or the fixture path, verified with `verify_delivery_audio`, and the line recorded beside probe-2's table in Appendix A (row E5). Probe-2 measured it once at **127.6 s**, integrated **−1 400**, true peak **−304**; the hand run confirms the raster-invariance the contract relies on, on the tree as committed.
3. **The real-harness `audio-workflow-v7` run** (§7.10): `KINEWRIGHT_EVAL=1 cargo run -p kinewright-agent --bin kinewright-eval -- --suite audio-workflow-v7 --harness claude-code --samples 3`, the blind review by a person answering the six questions of §8.4 at one listening level, `--score-review`, and the M40 gate of §8.6. `docs/EVALS.md` says "pending real-harness run" until then.
4. **The track `AUTOMATION` section heard.** §6.2 is proven at builder level; a track fader ride drawn in the running app and heard against the bus-curve alternative (A13 says they are bit-identical) closes it.
5. **The Windows lines.** The first green Windows CI run's printed `AU6_*` margin lines are transcribed into Appendix A row W with the run id — recorded, never gated (N1 Q10). **Still open:** no Windows run has ever executed an AU6 test. The four failures that aborted the job before it could are fixed here (§0.3 review finding 2), so the next run is the first that can reach them; until its lines are transcribed the row stays empty and no AU6 claim rests on it.

AU6 is complete only when a mixer can watch five ordinary jobs — balance two voices over a bed that ducks, match a quiet guest to a loud one and tame the swing, clean hiss, hum and clicks off location dialogue after filling a hole with the room's own tone, cut between two angles without touching the recorder's track, and deliver the same programme at two loudness targets — go through Kinewright by hand *and* by agent *and* by model, see every objective claim discharged by a build that runs on both operating systems with no model, no network and no audio device, and be asked only the six questions a machine has no business answering.

---

## 14. Risks

- **The de-click floor was the last constant measured, and it is measured on a different document from the click count.** `AU6_DECLICK_ERROR_DROP_MIN_TENTH_DB = 120` rests on probe-3's nine fresh-engine runs (drift 0.000) of the node **alone** against the click-free programme; the same instrument on the canonical chain reads 4.5 / −116.9 / 0.5 (§0.2 item 52). It is live because an implementer who writes row 14 against the chain, or against the voice-only clean, gets a number that cannot clear any floor and will be tempted to widen. Mitigation: §4(c)(3) names the fourth document, the reference buffer and the stem point; the fixture asserts the declick-only stem is bit-identical to `process_buffer_static`'s output so the product path and AU5's route cannot drift apart silently; the residual is **noise-limited** (probe-3 §1.5: 2.45× the in-span noise, 78 tenth dB under the 336.7 in-span-noise ceiling), so a change to the noise level moves it and the S2 level fixture catches that first. Fallback: re-baseline once with both numbers recorded, never widen after a red build.
- **The probes' cells are re-measured by the implemented fixtures, and the base rate for a moved number is high.** Probe-2 overturned N2's duck population (window 5 → 4), a hum constant by a factor of ten (250 → 2 400, a tenth-dB figure under a hundredths name), (d)'s failing direction, and the 1080p lane — four amendments from five tasks. Probe-1's first duck depth (890) and passthrough deltas (412 / 98) were superseded by probe-2's geometry. What the code will re-measure is every §4.1 row on `au6_sources`' buffers rather than the probe's scratch generators. Mitigation: §12 step 4's barrier; the manifest may hold no placeholder; every margin is printed. Fallback: re-baseline once, both numbers recorded in §0.3 — never widen after a red build.
- **Wall clock — the projection below was measured and missed by 7×; R23 carries the live numbers.** Measured by probe-1 §9: `mix_levels` **8 829 ms** over one 12 s document and **1 402 ms** over one 2 s turn; `mix_window_levels` **266–990 ms** whole-programme; `audio_qc` **1 626–2 391 ms**; `audio_repair` **465 / 3 900 ms** bare / three-node; a normalized 8 s 320×180 encode **13 898 ms** plus **941 ms** verification. Projected at contract time: media lane ≈ 90 s with (d)'s leg cut, agent lane ≈ 60 s, so **≈ 150 s** added to `cargo test --workspace` on top of the existing ≈ 100 s audio floor (media lib 51.8 s, `au3_fixtures` 49.0 s; N0.3(c), N0.7(f)). **Live (R20, R40, R23):** media lane **437 s** locally at `--test-threads=1` and **667 s** on Linux CI, agent lane **134 s**, and the whole Linux job **8 m 58 s → 19 m 27 s** — AU6 adds ≈ **10.5 minutes** per Linux run, not 150 s. Nothing is red: `ci.yml` has no `timeout-minutes`, and `AU6_MEDIA_LANE_BUDGET_SECONDS = 180` / `AU6_AGENT_LANE_BUDGET_SECONDS = 120` are recorded, never gated, never widened. **The call pattern is still normative and is why it is 10.5 minutes rather than half an hour** (§10.3): one `mix_levels` per authored turn, one whole-programme `mix_window_levels` render per point, `audio_qc` for LRA. The cut order (§12) buys ≈ 21 s of encode against a 629 s delta, so it is not the lever; §13 now carries an item that owns the lane cost and names the one that is.
- **Every encoded number is AAC noise at the centi-unit and a different FFmpeg on Windows.** Probe-2 saw the true peak move **1 centi-dBTP** across rasters; the thinnest encoded margin is the streaming lane's **204 against 100** (2.04×). An encoder change moves that by centi-dBTP, not by a hundred, but Windows CI uses a **different FFmpeg package** — `System233/ffmpeg-msvc-prebuilt ffmpeg-8.0.1-r3` (`scripts/setup-ffmpeg.ps1:12-13`) against Linux's `mifi/ffmpeg-builds 8.0-1` (`scripts/setup-ffmpeg.sh:27-28`) — so the first Windows run **is** the measurement for rows 21, 22 and the separation. Mitigation: floors at 2×, printed; the same document is byte-identical between the two exports so a drift is diagnosable per lane. Fallback: a per-OS note in the constant's doc comment, never a per-OS constant (§10.6).
- **Windows is unmeasured for every AU6 number, and one of them is an exact `assert_eq!`.** No Windows measurement of any AU3 or AU5 audio lane exists in-tree, only "CI green" (N0.5(c)). The most exposed rows: dialogue-over-bed at **2.03×**, true peak at **2.04×**, the seam's exact zero on an `f64`, and the **31-band exact pin**, whose Linux drift was exactly zero over three fresh engines but whose FFT is scalar `f64` with cross-OS agreement deliberately unasserted (AU3 E24, N0.5(f)). Mitigation: the worst band is **printed** even though the pin is exact (§2.5), so a Windows delta is a number in a log, not a mystery. Fallback: if Windows shows a nonzero band delta, the pin gains a ceiling re-baselined once from that measurement with a per-OS note — A19's rule in reverse — and never a per-OS array.
- **Two harness quirks read as regressions.** `cargo test -p kinewright-media --lib -- <filter> --test-threads=2` segfaults reproducibly; `cargo test -p kinewright-agent --test mcp_server <filter>` exits SIGSEGV **after** all tests report ok, and on 2026-09-09 the unfiltered binary did so inside `cargo test --workspace` while another cargo test was live (N0). AU6 adds six mcp tests carrying exporters and one shared media engine, which is more of exactly the surface that trips both. Mitigation: §12's gate rule (no concurrent cargo test), rerun alone, default thread count; reviewers are told in the commit message. An AU6 twin of `cc7_every_color_fixture_builds_a_valid_document` (`crates/kinewright-agent/src/bin/kinewright-eval.rs:7580-7581`), if written, inherits its `#[ignore]` for the same teardown reason.
- **The eval schema change ripples.** `EvalDefinition` gains `audio` at **26** literals (20 + 6, S10) and `EvalDeliverableSpec` gains a field at **twelve**; the tree does not compile between §12 steps 9 and 10. `published_v5_…` uses exhaustive `matches!` lists over existing `EvalAssertion` variants (CC7 §14), so **AU6 adds only new variants and touches no existing one**. The exporter on the eval server is a shared-runner change to all six suites (§7.8); the `normalize_to_profile_target` flag is off at every existing literal (§7.7); a forgotten `is_packaged_benchmark` arm **overwrites `docs/EVALS.md`** (§7.1). Mitigation: `au6_v6_deliverables_are_byte_identical_without_normalization`, `au6_the_eval_server_carries_an_exporter` and `au6_audio_workflow_suite_is_a_packaged_benchmark` each pin one of the three, both directions; `measurements` is `skip_serializing_if` so `results.jsonl` is byte-identical without them (§9).
- **The product change is the largest app edit an evaluation slice has made.** 92 `MixerSelection` references to sweep, `chain()` changing to `Option`, a new `AutomationTarget` variant, 300–450 lines. It is live because a mistake here is a **product** regression in a slice that promised none. Mitigation: app-only and `pub(crate)` (§9(11)); no ledger byte, pinned by `cc7_the_agent_surface_is_unchanged_by_this_slice`; the retired tooltip asserted absent; all three DESIGN.md pin loops inventoried (S11); and §6.6's fallback is a checked sentence, so slipping it is safe.
- **The audio evidence block is on Part B's critical path and cannot be cut.** Six of seven variants read it (§7.6). Mitigation: its own implementer (P), sized 400–600, the both-direction plumbing test `au6_audio_evidence_is_computed_where_the_analysis_is_alive`, and the `errors` vector that makes "unmeasurable" fail its own assertion with a reason rather than pass vacuously.
- **The `include_str!` inventory couples four crates at compile time.** A renamed test in the agent, app or eval sources fails a **media** test, and it is why `AU6_EVAL_TESTS` is `[&str; 0]` at the Part A commit and `[&str; 12]` at Part B. Mitigation: the inventory is the last step of each part (§12), and the both-direction assertion names the offending test.
- **Two recorded artefacts will read as regressions to a maintainer who has not read §4(c)(2).** The 50 Hz `mains_hum_present` finding *appears* after repair and the fourth harmonic's excess *rises*. Mitigation: both are in the manifest as `reported_not_gated` with the reason, and the fourth harmonic is asserted **not claimed**, so a future change that makes either behave "better" changes a recorded value and is noticed rather than a gate that nobody wrote.
- **`capture_refused` and the 20 s confirmation timeout.** The probe's agent lane included a **deliberate 20 s timeout** (probe-1 §10.1). AU6's `capture_refused` failing direction is driven by an **explicit refusal** from the approver, not a timeout, so the lane does not pay it; a test written the other way would add 20 s to every run.
- **The blind package is procedurally defeatable, and there is no A/B hold.** A reviewer who opens `blind-key.json` or recognises the two-voice-plus-bed programme is unblinded, and the two (e) artefacts are heard at whatever level the player produces. Mitigation: the leak test bounds what the tooling discloses on both surfaces (§8.5); §8.0 names what is not blinded; the README asks for one listening level. An honest limit.
- **Five scenarios in five crates, two commits, one hard barrier.** The chain in §12 is long. Mitigation: §2's single authority describes each scenario once; steps 6 and 7 are parallel; the cut order names two things that go without touching the exit gate; the two-part split puts every product change in Part A, where the app reviewers are.

---

## Appendix A — Measurement provenance

Every number in §2.3, §2.4, §2.5, §2.7, §2.8, §4, §4.1 and §4.2 is measured, not inferred. **Three** probes ran, all on 2026-09-10 at `11a6098` on a clean tree, all on Linux in the **debug** lane CI runs: **probe-1** measured every bracketed number of the brief on the critic's amended geometry and escalated E1–E12; **probe-2** measured the five quantities N2 named and escalated P2-E1..E3; **probe-3** measured the de-click error drop with AU5's instrument and escalated P3-E1..E3. Every row below carries its probe and its lane; **no cell is unmeasured**, and the only rows regenerated by the implementation are the two `performance` lanes and the Windows line.

**The standing rule.** `FIXTURE_MINIMUM_MARGIN = 2.0` (`crates/kinewright-media/tests/au3_fixtures.rs:59`, `crates/kinewright-media/src/au5b_fixtures.rs:72`) is restated in `src/au6_fixtures.rs` and is the bar for every row: a **floor** needs `measured / budget ≥ 2`, a **ceiling** needs `budget / measured ≥ 2`, an exact or zero term records **"infinite (measured exactly zero)"** with its failing-direction fixture as the bound, and every lane **prints its margin** whether or not it gates (margin-and-print). **No tolerance in this document may be invented, scaled, inherited from another lane, or conditioned on the operating system**: a Windows figure inside a budget but under 2× earns a per-OS note in the constant's doc comment, never a per-OS constant (§10.6). The manifest's `performance.measurement` block records the fields below per run.

**Probe-1** (`target/review/au6/probe-report.md`, 825 lines, §0–§12, E1–E12):

| Field | Value |
| --- | --- |
| Commit | `11a6098` (`docs: state the long-term goal in the roadmap`), clean tree; `git status --short` empty after clean-up (probe-1 §12) |
| Machine / OS | the RTX 3080 + lavapipe development machine, Linux (Arch); no GPU lane is used by any AU6 measurement |
| Toolchain | not recorded by the report; the drafting machine on the same day reads `rustc 1.98.0 (88d9e12ae 2026-08-18)` / `cargo 1.98.0 (797e8a9bc 2026-08-05)`, kernel `7.2.3-arch1-3`, and the manifest's `measurement` block records them again at §12 step 8 |
| Lane | **debug** `cargo test`, default thread count |
| FFmpeg (Linux) | `scripts/setup-ffmpeg.sh` — `mifi/ffmpeg-builds` **8.0-1**, SHA-256 `c201d31f…5cb1` (`:27-28`) |
| FFmpeg (Windows CI) | **a different package** — `System233/ffmpeg-msvc-prebuilt` `ffmpeg-8.0.1-r3`, SHA-256 `3399afab…e433` (`scripts/setup-ffmpeg.ps1:12-13`); **no AU6 number has been measured with it** |
| Probe harness | `crates/kinewright-media/tests/probe_au6.rs`, a `#[cfg(test)] mod probe_au6_stems` appended to `crates/kinewright-media/src/export.rs` (for `pub(crate) mix_audio_stems`), `crates/kinewright-agent/tests/probe_au6.rs` driving the **real MCP endpoint**; all three removed, copies kept under `target/review/au6/probe/` (gitignored) |
| Geometry | the critic's amended geometry: 25 fps, 48 kHz stereo, 320×180, 300 frames (312 for (c)), the §2.3 turn table, window = hop = 200 ms |
| Runtime | 78.3 s (a), 53.7 s (b), 10.8 s (c), 41.6 s (d), 140.8 s (e), 24.4 s mux, 1.9 s stems, 78.9 s agent lane incl. a deliberate 20 s confirmation timeout (probe-1 §10.1) — exploratory, more variants than AU6 runs |
| Not measured | the de-click error drop (probe-3, below); Windows (row W) |

**Probe-2** (`target/review/au6/probe2-report.md`, 450 lines, tasks 1–5, P2-E1..E3), same commit and lane:

| Field | Value |
| --- | --- |
| Commit / OS / lane / FFmpeg | as probe-1 |
| Probe harness | `crates/kinewright-agent/tests/probe_au6.rs` (task 1, real endpoint), `crates/kinewright-media/tests/probe_au6.rs` (tasks 2, 3, 5), `#[cfg(test)] mod probe2_au6_multicam` appended to `export.rs` (task 4); all removed, copies under `target/review/au6/probe/` |
| Runtime | 5.7 s (duck populations), 3.6 s (three profile learns + both bounds), **350.2 s** (four delivery rasters — never repeated in the suite), 19.3 s (multicam), 3.2 s (harmonics) — probe-2 §7 |
| Not measured | the de-click error drop (probe-3, below); Windows |

**Probe-3** (`target/review/au6/probe3-report.md`, 319 lines, §0–§4, P3-E1..E3), the de-click error drop:

| Field | Value |
| --- | --- |
| Commit / OS / lane / FFmpeg | as probe-1: `11a6098`, clean tree, Linux, debug lane, lavapipe default lane with no GPU involved |
| Probe harness | one throwaway module `target/review/au6/probe/probe3-media-src-export.rs`, appended to `crates/kinewright-media/src/export.rs` as `#[cfg(test)] mod probe3_au6_declick` (for the `pub(crate)` `mix_audio_stems`, `process_buffer_static` and `detect_clicks`), run as `cargo test -p kinewright-media --lib probe3_au6_c_declick_error_drop -- --nocapture`, reverted with `git checkout`; raw output `probe/probe3-output.txt` and `probe/probe3-output-v2.txt`; `git status --short` empty (probe-3 §4) |
| Fixture | probe-2's (c) synthesis verbatim (`probe/probe2-media-tests.rs:787-831`); three buffers kept in memory — `clean` (voice), `click_free` (voice + noise + hum), `degraded` (`click_free` + clicks) — only `degraded` written to disk; four bus chains `bare` / `full` / `no_declick` / `declick_only` |
| Instrument | AU5's `au5_the_declick_fixture_drops_its_error` verbatim (`crates/kinewright-media/src/audio.rs:11409-11449`): `200·log10(rms(degraded − click_free) / rms(repaired − click_free))`, whole programme, both channels, 1 198 080 samples; product path `mix_audio_stems` post-effects `Bus(Dialogue repair)` stem, cross-checked bit-identical against `process_buffer_static` |
| Runs | **9** fresh `FfmpegMediaEngine` runs (3 on probe-2's click, then **three click shapes × 2**), every printed quantity identical to the last digit, `spread_tenth_db = 0.000`; ≈ 9.5 s per run |
| Alignment | the `bare` stem is bit-identical to the authored buffer (`max_abs = 0.000e0`, scale `1.000000000`), so in-memory references subtract directly |
| Not measured | the silence detector's 10 ms windows around an AU5-shaped in-gap click (moot: the shape is withdrawn, A20); Windows |

**The regeneration rule for the two `performance` lanes.** The media and agent lanes (rows C10, C11) are the only Appendix A numbers regenerated by every implementation run: re-measured once at §12 step 4 and 6, once more at §12 step 8 on the finished Part A tree, pasted into `au6_manifest.json`'s `performance` block, recorded in §0.3, and **never gated** (`ci.yml` has no `timeout-minutes`). A later commit that moves either lane past its recorded budget re-measures and records; it does not edit the budget constant without a §0.3 line.

**Rows.** `#` A1–A22 are §4.1's rows in order; S1–S3 the three source budgets; C the other constants; E the evidence and cost figures the body cites; W Windows.

| # | Constant / quantity | Value | Measured | Probe | Lane | Regenerates when |
| --- | --- | ---: | --- | --- | --- | --- |
| A1 | `AU6_INTERVIEW_DIALOGUE_OVER_BED_MIN_LU_HUNDREDTHS` | **400** | **812** (Dialogue −2 158, Music −2 970); un-ducked **−388** | probe-1 §1.2 | media, `mix_levels` once per turn on (a)'s canonical document | (a)'s levels, the trims, the pinned ducking arguments, or K-weighting change |
| A2 | `AU6_DUCK_DEPTH_MIN_HUNDREDTHS` | **400** | **1 187** at window 4 (1 011 at window 5); un-ducked **−14** | probe-2 §1 (supersedes probe-1 §1.3's 890 under the earlier population) | agent, real `plan_audio_ducking` then `mix_window_levels` at `Bus(Music)` | the pinned ramp arguments, the 1 s gap, the 200 ms grid |
| A3 | `AU6_VOICE_MATCH_MAX_LU_HUNDREDTHS`, (a) | **150** | **5** at `+37`; **365** un-mixed; **235** at the nominal `+60` | probe-1 §1.4 | media, `mix_levels` over each speaker's turns | the carrier bands or levels |
| A4 | the same constant, (b) | **150** | **50** through c1m; **1 442** un-trimmed | probe-1 §2.2 (transcribed probe-2 §6) | media, `Bus(Voice A)` / `Bus(Voice B)` | the chain constants or trims |
| A5 | `AU6_PODCAST_SPREAD_REDUCTION_MIN_HUNDREDTHS` | **450** | **978** (1 482 bypassed → 504); bypass **0** | probe-1 §2.1–2.2 | media, `mix_window_levels` at `Bus(Voice B)` | the 1 Hz / 18 dB ride or the chain |
| A6 | `AU6_PODCAST_LRA_MAX_LU_HUNDREDTHS` | **300** | **50**; makeup-less **1 274**; un-trimmed **1 434** | probe-1 §2.2 | media, `audio_qc` over the programme | the makeup gain |
| A7 | `AU6_NORMALIZATION_PREDICTION_MAX_LU_HUNDREDTHS` | **25** | **1** (−2 299 against −2 300; −1 399 against −1 400) | probe-1 §7.2 | agent, real `plan_audio_normalization` at both targets | the planner's arithmetic |
| A8 | clipping, every scenario | **0 runs** | **0 / 0** on every variant, true peak **−1 080** | probe-1 §1.5, §2.2 | media, `audio_qc` | any level change |
| A9 | `AU6_LEARN_GAP_BELOW_SILENCE_MIN_HUNDREDTHS` | **250** | **526** (learn gap −4 026; other gaps −3 978); the brief's levels ≈ −3 400 | probe-1 §3.1 | media, asset RMS of (c)'s gaps | (c)'s noise/hum levels or `DEFAULT_SILENCE_THRESHOLD_DBFS_HUNDREDTHS` |
| A10 | `AU6_REPAIR_SNR_GAIN_MIN_HUNDREDTHS` | **500** | **1 142** (2 041 → 3 183; probe-2 re-read 1 143); neutral profile **132** | probe-1 §3.4 / §7.4, probe-2 §5 | media, `audio_repair` at `Bus(Dialogue repair)`, planner chain | AU5's repair constants or the learned profile |
| A11 | `AU6_HUM_DROP_MIN_DB_HUNDREDTHS` | **2 400** | **4 970** (9 163 → 4 193); no hum node **1 tenth dB** | probe-2 §5 (probe-1 §3.4 read 5 108 on its run; the tighter figure is adopted) | media, `hum_60_excess_db_hundredths` | notch depth or Q, `REPAIR_HUM_HARMONIC_COUNT` |
| A12 | `AU6_HUM_HARMONIC_DROP_MIN_DB_HUNDREDTHS` | **500** | **1 096 / 1 706 / 2 272** at 60 / 120 / 180 Hz; 240 Hz **−103** (rises) | probe-2 §5 | media, `hum_60_harmonic_excess_db_hundredths` | as A11 |
| A13 | clicks after repair | **0** | **0** (before **12**); neutral profile keeps **12** | probe-1 §3.4, probe-2 §5 | media, `click_count` | the de-click threshold |
| A14 | `AU6_DECLICK_ERROR_DROP_MIN_TENTH_DB` | **120** | **258** (258.8; `before_rms 0.003189`, `after_rms 0.000162046`); window-invariant at ±1 / ±10 / ±40 ms; no-declick document **0.0** with `max_abs = 0`; 2.44× on AU5's ±0.9 shape and 2.02× at ±0.5 | probe-3 §0.1, §0.3 | media, `mix_audio_stems` on the declick-only document against `au6_c_click_free_track()`, bit-identical to `process_buffer_static` | the click shape (row C12), the noise level (the residual is noise-limited, probe-3 §1.5), or the de-click detector |
| A15 | `AU6_REPAIR_SPEECH_LOSS_MAX_DB_HUNDREDTHS` | **300** | **101** (−2 418 → −2 519) | probe-1 §3.4 | media, `mix_window_levels` over the four turns | the denoise reduction |
| A16 | profile band pin, exact `assert_eq!` | **0** | drift **0** over three fresh engines; the 31 values of §2.5 | probe-2 §2.1 (first read probe-1 §3.3) | media, `mix_noise_profile` over `274..312` at `Bus(Dialogue repair)` | the bed, the learn range, `NOISE_PROFILE_PERCENT`, the integrator or the FFT — and Windows (row W) |
| A17 | `AU6_PROFILE_LEAKAGE_ALLOWANCE_TENTH_DB` | **60** | worst excess **27** at band 5; minimum **−94** at band 1 | probe-2 §2.3 | arithmetic on the probe-2 profile, Hann main lobe `MAIN_LOBE_BINS = 4.0` | as A16, or `MAIN_LOBE_BINS` |
| A18 | room-tone seam | **0.0** | **0.0**; `track_gaps` `Some(vec![])`; clips `[(1,0), (3,160), (2,175)]` | probe-1 §7.5 | agent, real `capture_room_tone` + `plan_room_tone_fill` | AU5's fill arithmetic |
| A19 | master stem identity | **0 samples** | **1 152 000 / 1 152 000** identical after the cuts and after an angle ripple; master split + ripple diverges at **576 000** | probe-1 §4.5, probe-2 §4.1 | media, in-crate `mix_audio_stems` | any mix summation change |
| A20 | `AU6_MASTER_PASSTHROUGH_MAX_LU_HUNDREDTHS` | **50** | **0** (−2 670 both); unmuted scratch **600** | probe-2 §4.2 (probe-1 §4.4 read 0 at −2 015 on the muxed-scratch construct, failing 412 / 98) | media, `mix_levels` | the master level |
| A21 | `AU6_DELIVERY_DEVIATION_MAX_LU_HUNDREDTHS` | **25** | **0 / 0** at 8 s (worst **1**, the 12 s source master); un-normalized **122 / 778** | probe-1 §5 | media, encode + `verify_delivery_audio`, 320×180, 8 s | the FFmpeg package, AU3's normalization |
| A22 | `AU6_DELIVERY_TRUE_PEAK_MARGIN_MIN_HUNDREDTHS` | **100** | **204** (streaming, −304); source master **1 104** | probe-1 §5, probe-2 §3 | as A21 | the AAC encoder (row W) |
| S1 | `AU6_TARGET_SEPARATION_TOLERANCE_LU_HUNDREDTHS` | **25** | **0** at 8 s (900), **1** at 12 s (901); one profile twice **0** separation | probe-1 §5 | media, arithmetic on two verifications | as A21 |
| S2 | `AU6_AUTHORED_LEVEL_TOLERANCE_HUNDREDTHS` | **25** | worst **10** ((c) voice −2 390); (a) 2 / 0 / 0; (b) 5 / 0 | probe-1 §1.1, §2.1, §3.1 | generator arithmetic | any generator change |
| S3 | `AU6_VOICE_BAND_SEPARATION_BANDS` | **1** | **3** (bands 17 and 20) | authored; confirmed probe-1 §1.1 | media, `mix_spectrum` at `Track(A1)` / `Track(A2)` | the carrier bands |
| C1 | `AU6_INTERVIEW_A2_TRIM_TENTH_DB` | **37** | delta 5; the pair 3.65 LU apart in LUFS against 6.00 dB in dBFS | probe-1 §1.4 | media | the bands or K-weighting |
| C2 | `AU6_PODCAST_A_TRIM_TENTH_DB` / `_B_` | **72 / −72** | voice delta **1** bypassed, **50** through c1m | probe-1 §2.2 | media | as C1 |
| C3 | `AU6_A_DUCK_ATTACK_MS` / `HOLD` / `RELEASE` / `DEPTH_TENTH_DB` | **150 / 200 / 400 / −120** | the shipped defaults, **16** keyframes | probe-1 §7.1, probe-2 §1 | agent, real endpoint | `plan_audio_ducking`'s defaults (`server.rs:9849-9859`) |
| C4 | `AU6_A_DUCK_KEYFRAMES` and the detected windows | the 16 keys of §2.6 | identical in both probes; windows `{1,54} {77,129} {151,204} {227,279}` | probe-1 §7.1, probe-2 §1 | agent | `timeline_silences` or the planner |
| C5 | `AU6_C_LEARN_PROJECT_RANGE` | **`274..312`** | spans `[(60,76), (124,140), (210,226), (274,312)]`, longest by 21; one frame moves band 0 by **29** | probe-1 §3.2 / §7.4, probe-2 §2.2 | media + agent | the detector's window or the bed |
| C6 | chain **c1m** | §2.6's nine constants | spread 504, reduction 978, voice Δ 50, LRA 50, true peak −1 609; c1 (makeup 0) 1 250 / 1 274; c4m 362 / 28 / 30 | probe-1 §2.2 | media | never by measurement — authored |
| C7 | `AU6_C_CLICK_COUNT` at its twelve frames | **12** | 12 detected | probe-1 §3.1 | media | authored |
| C8 | `AU6_ENCODE_PROGRAMME_SECONDS` | **8** (media lanes on the 200-frame (e) document; the agent lane's `a5a` is 12 s, B2) | all four BS.1770 quantities populated at 8 s; LRA **5** at both lengths; deviations 0 / 0 at 8 s against −1 / 0 at 12 s | probe-1 §5.1 | media | a shorter encode must re-find the LRA floor (§13) |
| C9 | `AU6_LEARN_MINIMUM_PROJECT_FRAMES` | **12** | `ceil(22 528 × 25 / 48 000)`; the planner's own minimum is 469 ms (`server.rs:9776`) | probe-1 §3.2 | arithmetic | `NOISE_PROFILE_MINIMUM_FRAMES` |
| C10 | `AU6_MEDIA_LANE_BUDGET_SECONDS` | **180** | **≈ 90 s** projected with (d)'s leg cut (≈ 105 s with it): (a) 20, (b) 18, (c) 11, (d) mix 9, (e) 30, stems 2 | probe-1 §10.2 | media, debug lane | **every implementation run** (regeneration rule above) |
| C11 | `AU6_AGENT_LANE_BUDGET_SECONDS` | **120** | **≈ 60 s** projected: a1 5, a2 9, a3 15, a4 5, **a5a 24** (probe-1 §5's 12 s row 21 005 + 1 419 ms plus ≈ 1.2 s overhead), **a5b 2** (queue and cancel, the overhead alone, not a measured cancel) | probe-1 §5, §10.2; B2/B3 | agent, debug lane | as C10 |
| C13 | `AU6_E_DUCK_KEYFRAMES` (B2) | the ten `at < 200` keys of C4 | probe-2's (e) document carried its helper's regenerated ten-key curve `(0,−120) (55,−120) (65,0) (72,0) (75,−120) (130,−120) (140,0) (147,0) (150,−120) (199,−120)` — 300-frame assets, 200-frame clips, duration 200 — differing from `AU6_E_DUCK_KEYFRAMES` only inside ramps by ≤ 1 frame | probe-2 §3 (`probe/probe2-media-tests.rs:913-995`, `:357-424`) | media, the two 8 s delivery lanes | re-measured at §12 step 4 on the pinned curve |
| C12 | `AU6_CLICK_SAMPLES` / `AU6_CLICK_AMPLITUDE_HUNDREDTHS` | **2 / 50** | the shape every probe used; `click_count` 12 on every run; repaired spans 12 × 4 sample frames starting at offset 0; `d2 = 2.0` against a ≈ 0.11 threshold | probe-3 §0.3, §1.5, §3.2 | media | authored; withdrawn shapes in row E20 |
| E1 | window alignment | — | `PROBE_A_ALIGNMENT range=50..75 frames=25 window_ms=200 windows=5`, `report.range` echoed verbatim | probe-1 §1.7 | media | `mix_window_levels`' anchoring |
| E2 | curve-owner equivalence | — | master and bus stems bit-identical for all three owners, maximum absolute delta `0.000e0`; track stem differs for the bus route only; **1.9 s** for both stem tests | probe-1 §1.6 | media, in-crate | any curve evaluation change |
| E3 | `Track(A3)` against `Bus(Music)` | — | **−1** at the track point for the bus owner; **890** at the bus for all three | probe-1 §1.3 | media | the stem order |
| E4 | (b)'s AM rate | — | bypass spread **377** at 4 Hz / 200 ms, **1 482** at 1 Hz / 200 ms, **1 689** at 1 Hz / 100 ms | probe-1 §2.1 | media | the grid |
| E5 | raster invariance (hand-run lane, §13) | — | 320×180 **14.7 s**; 1920×1080 **127.6 s**; 1080×1080 **77.6 s**; 1080×1920 **128.8 s**; integrated / momentary / short-term / LRA identical; true peak −304 / −303 | probe-2 §3 (350.2 s, run once) | media, encode at the profile's raster | the hand run; never the suite |
| E6 | (d)'s cut delivery leg | — | **21.6 s** (encode 20 145 ms + verify 1 448 ms) at 12 s; deviation 0, true peak −1 653 | probe-1 §4.6 | media | if restored (§13) |
| E7 | scratch and video-only reporting | — | muted `audible: false, None`; video-only `audible: true, None`; unmuted scratch **−2 965 / −2 968**; angles probe `Video 25/1 300 (320,180)` | probe-2 §4 | media | `MixLevelReport`'s shape |
| E8 | the 50 Hz artefact | — | `hum_50_excess` **0 → 829** (planner) / **1 072** (media chain) / **911** (probe-2's run); a second `mains_hum_present` | probe-1 §3.4, probe-2 §5 | media | recorded, never gated (§13) |
| E9 | the Goertzel `None` rule | — | **24** whole blocks at 599 040 sample frames, nothing `None`; under one block `None` and **`[]`** | probe-2 §5.2 | media | `HUM_GOERTZEL_BLOCK_FRAMES` |
| E10 | the percentile floor | — | **−4 071** against authored **−4 026**, within 0.45 dB; 39 % of (c) is noise-only | probe-1 §3.5 | media | the gap fraction |
| E11 | the profile's shape | — | bands 4–6 **13–20** above trend; bands 1–3 **30–40** below; monotone above band 8 at ≈ 0.4 dB per band against 1.0 analytic | probe-1 §3.3 | media | as A16 |
| E12 | the point-mass model | — | worst excess **225** at band 4, allowance would be **450**; noise-only bands **2–94** under the main-lobe bound | probe-2 §2.3 | arithmetic | as A17 |
| E13 | `plan_clip_fades` | — | first attempt proposed **nothing** (`"the track stem is silent over this clip's head or tail window"`); on `90..103` / `103..120` **1 / 1** frames, `window_project_frames = 1`, threshold −4 000 | probe-1 §7.3 | agent | the fade planner's skip rule |
| E14 | the typed codes reached | — | `revision_conflict` with `details.{actual_revision, allowed, expected_revision, field, observed, recovery_action}`; `capture_refused` with `details.{store_file_written:false, document_changed:false, reason, recovery_action}`; the two planner refusals as prose / `-32602` | probe-1 §7.6 | agent, real endpoint | the envelopes (§13) |
| E15 | the captured room tone | — | probes at **30 fps**, **44** asset frames / **1 466 ms**, 70 400 sample frames; capture 44 ms, fill plan 1 ms | probe-1 §7.5 | agent | the store's grid |
| E16 | mux recipe | — | `pcm_s16le` / `pcm_f32le` / `pcm_s24le` all mux, probe `AudioVideo`, export **6 729 / 6 836 / 6 886 ms**; audio-only export **accepted** | probe-1 §6 | media | the FFmpeg package |
| E17 | cost model | — | `mix_levels` **8 829 ms** / 12 s, **1 402 ms** / 2 s turn; `mix_window_levels` **266–990 ms**; `audio_qc` **1 626–2 391 ms**; `audio_repair` **465 / 3 900 ms**; stems **< 1 000 ms**; 8 s normalized encode **13 898 ms** + verify **941 ms**; synthesis **1 110 ms** | probe-1 §9 | media, debug | every run, informally |
| E18 | planner timings | — | `plan_audio_ducking` **3.2 s** incl. silence analysis; `plan_dialogue_repair` **2.6 s**; `capture_room_tone` + fill **45 ms** | probe-1 §7 | agent | — |
| E19 | (a)'s un-ducked bed level | — | Music **−1 770** un-ducked, **−2 970** ducked: a 1 200 centi-LU shift, of which the failing direction is **788** short of the 400 floor (N1) | probe-1 §1.2 | media | as A1 |
| E20 | the withdrawn click shape (P3-E1) | — | AU5's 8-frame ±0.9 click: clicked gaps **−36.39 dBFS**, **139** under −35.00 (**fails** the 250 floor), drop 293.3, SNR 2 057; at ±0.5: −38.65, **365** under (1.46×), drop 242.3; the 2-sample click: **−39.78**, **478** under | probe-3 §0.3, §2 | media, asset RMS of the three clicked gaps | never — recorded as the wrong model |
| E21 | the de-click contribution inside the chain | — | full chain vs clean **4.5** (±10 ms 71.6, ±1 ms 159.0); without declick **4.0**; the node's share **0.5 / 17.2 / 71.6**; full chain vs click-free **−116.9**; SNR bare / declick-only / full **2 040 / 2 040 / 3 183** | probe-3 §0.2 | media | recorded as `declick.contribution_in_chain`, never gated |
| W | **Windows** — every gated row above | — | **unmeasured.** The first green Windows CI run's printed `AU6_*` margin lines are transcribed here with the run id, recorded, never gated (N1 Q10) | the CI job | `windows-latest`, `System233` FFmpeg | each Windows run; a row under 2× earns a per-OS note, never a per-OS constant |
