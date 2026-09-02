# CC7 — Workflow evaluation

Status: as implemented 2026-08-28 (errata folded in — see §0.3)
Depends on: [CC0](ROADMAP-AND-WORKFLOWS.md), [CC1 managed SDR primary](CC1-MANAGED-SDR-PRIMARY.md), [CC2 scopes and matching](CC2-SCOPES-AND-MATCHING.md), [CC3 curves and wheels](CC3-CURVES-AND-WHEELS.md), [CC4 look management](CC4-LOOK-MANAGEMENT.md), [CC5 secondaries](CC5-SECONDARIES.md), [CC6 QC and managed delivery](CC6-QC-AND-MANAGED-DELIVERY.md), [M34 creator delivery verification](M34-CREATOR-DELIVERY-VERIFICATION.md), [M35 finished-cut benchmark](M35-FINISHED-CUT-BENCHMARK.md), [M36 agent runtime efficiency](M36-AGENT-RUNTIME-EFFICIENCY.md), [M40 generalization gauntlet](M40-GENERALIZATION-GAUNTLET.md)
Scope: **proving that six named colour workflows complete end to end over the CC0–CC6 surface — by a person, by a scripted agent, and by a model — with every objective claim discharged by an ordinary `cargo test` on both CI operating systems, and with the human reviewer left only the creative questions the roadmap's colour evaluation matrix asks.**

CC7 adds **no colour feature and no MCP tool**. Every measured quantity comes from a core function or a tool response that exists at `99faee3`. CC1's input/working/monitoring/delivery contract, CC2's matching semantics, CC3's node order, CC4's asset store, CC5's matte and tracking boundaries, and CC6's QC engine, budgets, and verification are preserved verbatim: CC7 *consumes* them and records the margins.

CC7 **evaluates**. It never proposes a fix, never widens a CC6 budget, never re-baselines a codec tolerance, and never renames or deletes an existing test.

The words **must**, **must not**, and **may** in this document are normative.

---

## 0. Change log

### 0.1 Changes from the brief

The 2026-08-27 design brief's rulings D1–D6 were transcribed into draft v1. Probe-1 then measured every bracketed number and escalated seven findings (E1–E7); the orchestrator's amendments **A1–A11** (`target/review/cc7/amendments.md`) supersede the corresponding brief text and are folded in below. Each line is a design change, not an edit.

- **A1 (E1) — the chart band becomes genuinely achromatic.** `CC6_NEUTRAL_CHART_CODES` is not a neutral chart: six of its twelve entries are saturated primaries. Row `y 36..52` now carries **twelve achromatic patches** `[0, 11, 24, 48, 72, 104, 128, 152, 180, 208, 242, 255]`, and a separate **primaries band** at `y 56..72` carries **five** saturated patches `[0,255,0] [0,0,255] [0,255,255] [255,0,255] [255,255,0]`. The **pure red `[255,0,0]` is omitted**, because the derived `product_red` qualifier (hue `35 865 ± 1 500` + `1 000` cd softness, probe P5) captures it and made (d)'s exact containment unreachable; magenta (30 000 cd) and yellow (6 000 cd) sit more than 2 500 cd away and are kept; the blue primary is kept because it is the pixel that clips in (b2). This resolves draft v1's OPEN-3: (a)'s `plan_shot_match` ROI and spread statistic are now over **all twelve** chart patches.
- **A2 (E2) — no `.cube` size reaches one code.** `CC7_LOG_INVERSE_MAX_CODE = 12` (measured 5, 2.4×) at lattice size **65** with the output clamp kept. The size rule is restated in §3.4. The black patch's 4-code error is a property of the curve (not invertible at 0), recorded in §2.4.2 — this resolves draft v1's OPEN-6.
- **A3 (E3) — (e)'s exact gate is the `deep_shadow` patch.** The ramp's out-of-gamut column count is not analytic through the limited→full decode (measured 1 608 against 1 568 predicted from source codes). The exact gate is `out_of_gamut_pixel_count == CC7_LOOK_DEEP_SHADOW_OUT_OF_GAMUT_PIXELS = 192` on a `deep_shadow` ROI; the whole-raster count and basis points are **reported, never gated**.
- **A4 (E4) — the tracking recipe is re-cut (superseded in detail by A12–A14, A17, A18).** `track_clip_region` sets `previous = current` after every sample (`server.rs:5292-5315`), so the template is the *previous sample*, not frame 0: an occlusion spanning several samples scores 10 000 in its interior. The occlusion becomes frames **43..=47** (one sampled frame inside) and the expected low-confidence set is "entering and leaving". Probe-1 ran out of wall time before the live-endpoint test, so draft v2 left every tracking number, the sampled-frame list, the motion amplitude and the window half sizes unpinned; probe-2 has since measured all of them, and A12–A14/A17/A18 below replace this ruling's predictions with the measurements (the expected low-confidence set is `{47}`, not "entering and leaving"; the floor is 8 500, not 8 000). This superseded draft v1's OPEN-1, OPEN-2 and OPEN-4; probe-2 has since measured the whole family and A12–A14, A17 and A18 replace the placeholders (below).
- **A5 (E5) — (a) gate (5) is a compositor-layer parity case.** No document-level `LinearParityMetrics` helper exists; `assert_linear_parity` compares `Compositor::render_working` against a CPU reference over a `WorkingFrame`. Gate (5) is now stated as CC1/CC3/CC4 state it (`cc3_fixtures.rs:517`, `cc4_fixtures.rs:898`), reusing `LINEAR_CPU_GPU_{MAX,P99,MEAN}` unchanged, plus document-level render determinism recorded as **evidence** (probe-1 measured max/P99/mean `0 / 0 / 0` over 172 800 samples, 0 non-finite).
- **A6 (E6) — every verified export carries an Info exception.** `delivery_tag_not_representable` on `white_point` is present on both depths on every lane, because H.264 has no white-point field. The (g) gate is `technical_pass && within_budgets && every exception code ∈ CC7_DELIVERY_ALLOWED_INFO_CODES`, and the human question fires only on a `Warning`.
- **A7 (E7) — the feather band is a fraction of the window, not of the raster.** `feather_basis_points` scales the normalized distance field, so (d2)'s analytic model is the **discrete pixel-centre count** of §4(d)(4); the continuous-area formula of draft v1 was wrong by 35 pixels and is named as the wrong model. `CC7_FEATHER_PARTIAL_TOLERANCE_PIXELS = 4`.
- **A8 (probe P1/P2) — the (a)/(b) budgets are pinned.** `CC7_MATCH_NEUTRAL_SPREAD_MAX_CODE = 6` (measured 2, 3.0×) — **superseded by A15: 5**; `CC7_MATCH_LUMA_MEAN_MAX_CODE_MILLIONTHS = 5_000_000` (measured 2 166 667, 2.3×). Failing directions: the spread gate fails on **corrected C2** (17 — **superseded by A15: 19**) and the luma gate on **unmatched B** (−14 250 000). **(b2)'s prose is corrected** (and **superseded again by A16** on the amended scene): the pixels that clip are the **blue primary patch**, not the chart whites. `CC7_UNRECOVERABLE_RESIDUAL_SPREAD_REPORTED_CODE = 17` and the corrected-C2 skin `in_band_basis_points = 9 411` are reported, not gated.
- **A9 (probe P3) — the (c) budgets are pinned.** `CC7_LOG_FIRST_PERCENTILE_MIN_CODE = 20`, `CC7_LOG_P99_MAX_CODE = 200` (carrier 28 / 167 against cam A's 10 / 242) — **superseded by A21**, which restates both in the 16-bit unit the tool actually publishes; the 8-bit numbers survive as prose equivalents only. `import_lut_asset` requires the project-path handle: the branch-server pattern at `mcp_server.rs:1351` is copied rather than reinvented.
- **A10 (probe P7) — both depths, for every scenario.** CC6's `DeliveryBudgets` are reused verbatim (worst margin 2.16× at Eight); the Ten lane costs the same ~3.75 s as Eight, so **the Ten lane is removed from the cut order** and the CI delivery leg is budgeted at 90 s on Linux.
- **A11 (probe P8) — `cc7_sources` is `pub` and documented as test-support-only.** `run_ffmpeg` panics on a missing binary and on a nonzero exit; the module doc says so, exactly as `test_support`'s does.

- **A12 (E8) — the tracker never recovers from an occlusion, so the tracked range stops at it.** Probe-2 measured every post-occlusion sample reporting `confidence_basis_points = 10 000` while the observed centre stayed **frozen** at its pre-occlusion value — up to **5 176 bp** (165 px) from the subject by frame 74. The (f) call is therefore `start_local_frame 0, end_local_frame 48, step_frames 5, search_radius_percent 10, max_width 256`; the tool's even-distribution rule gives `CC7_TRACK_SAMPLE_FRAMES = [0, 4, 9, 14, 18, 23, 28, 32, 37, 42, 47]`; the occlusion stays `43..=47`; and the gate is `low_confidence_samples == {47}` exactly. **The brief's `(100, 40)` amplitude is kept** — probe-2 built the drafter's slower `(60, 30)` variant and measured it worse on every gated term. §13 records the no-re-acquisition boundary.
- **A13 (E9) — the (f2) total-loss recipe must start on the subject.** `43..48`, `42..48` and `39..51` all score `10 000` on every sample and refuse nothing, because the first sample is seeded from the window's *stored static* centre and a range starting inside the occlusion seeds the template on flat surround. The recipe is `start 0, end 48, step_frames 47` → samples `{0, 47}` → survivors 1 → `tracking_confidence_too_low` with `surviving_samples 1, total_samples 2, minimum_surviving_samples 2`. The same call at the 5 000 default does **not** refuse, so both directions are asserted and the floor is load-bearing.
- **A14 (E10) — `CC7_TRACK_MIN_CONFIDENCE_BASIS_POINTS = 8_500`**, not draft v2's predicted 8 000: occluded maximum **7 411**, clean minimum **9 740**, so 8 000 would have left only 589 bp of headroom. 8 500 gives **+1 089 / −1 240 bp** on a 2 329 bp separation — above A4's 2 000 bp bar, so the occlusion sub-gate is **not** dropped. `CC7_TRACK_TOLERANCE_BASIS_POINTS = 200` (CC5's, reused) against a measured worst clean raw observation error of **49 bp**, a 4.08× margin.
- **A15 (E11) — `CC7_MATCH_NEUTRAL_SPREAD_MAX_CODE = 5`, not 6.** On the amended twelve-patch band the *unmatched* cam B measures **exactly 6**, so a `≤ 6` gate would have passed its own failing-direction fixture. Matched cam B measures **2** (2.5×); corrected C2 measures **19**, so `CC7_UNRECOVERABLE_RESIDUAL_SPREAD_REPORTED_CODE` is **19**, not draft v2's 17 (which was the six-patch grey ROI). `CC7_MATCH_LUMA_MEAN_MAX_CODE_MILLIONTHS = 5_000_000` stands: measured **−1 381 567**, a 3.62× margin.
- **A16 (E12) — (b2)'s clipping prose is corrected again.** On the amended scene **672 pixels (116 bp)** go over range, **on the blue channel only**: the blue, cyan and magenta primary patches (384), the **white achromatic patch** (128) and the ramp's brightest columns (160). Draft v2's "the blue primary clips, not the whites" was true of the pre-amendment scene and is false of this one, because A1 added a 255 achromatic patch. Reported figures, not gated; per-node attribution still names the primary alone.
- **A17 (E13) — every tracking gate reads `observations[]`, never `curves`.** The smoothed curve's final keyframe is **746 bp** off whenever the last sample is dropped, by the tool's published `window_stabilization.known_systematic_lag`. The containment gate uses a **1.5× window** (`half_width 563`, `half_height 1_000` bp = 18 px) at every surviving sample **except the named final keyframe, frame 42**; the measured required half-extents **14.77 / 12.88 px** and their **3.23 / 5.12 px** margins are reported, not gated.
- **A18 (E14) — `search_radius_percent = 10`, pinned with its reason.** Every observation, confidence and keyframe was bit-identical at 10 % and 25 % on both amplitudes; the per-sample motion is ≤ 25 thumbnail px, inside the 10 % radius of 25.6 px. CI runs one radius.
- **A19 (E15) — `deep_shadow`'s ROI is `y_basis_points = 4223`.** The naive `4222` (`76 · 10000/180` truncated) resolves to pixel `y 75, h 17` — 204 pixels, not 192. Every patch ROI uses the `ceil`ed start and §11.2.1 asserts the resolved pixel rect. (d) is confirmed at **192 / 192 / 0 / 0** on the amended scene and (e)'s whole-raster gamut count is **1 480 px / 256 bp**.
- **A20 — what probe-2 did not measure.** Scenario (c), C1's proposal, the (b2) per-node delta, the (a) skin-band and chroma-spread numbers, and the (b1) residual were **not** re-measured on the amended scene. **Probe-3 has since closed scenario (c) in full** (A21, A22); the rest remain marked **"(P1; re-measured at §12 step 5)"** and must be re-run before the manifest is authored. The budgets stand as stated.

- **A21 (unit hazard) — the (c) signature gate is stated in the unit the tool publishes.** `analyze_color_shot` serializes `scope_statistics.luma.first_percentile` / `.ninety_ninth_percentile` as **16-bit** codes (`8-bit × 257`, `scopes.rs:576-586`, `:1330-1339`); `mean_code_values.luma` is an 8-bit *mean* and is the wrong field. The constants become **`CC7_LOG_FIRST_PERCENTILE_MIN_CODE16 = 5_140`** and **`CC7_LOG_P99_MAX_CODE16 = 51_400`** (carrier `7 196 / 42 919`; cam A `2 570 / 62 194`, failing both). A9's bare `20` / `200` survive only as 8-bit prose equivalents in the manifest. Left uncorrected the p1 gate would have passed on every source and the p99 gate failed on every source.
- **A22 (E16) — the lattice size is pinned, and the sweep is evidence.** `CC7_LOG_CUBE_SIZE = 65` with `CC7_LOG_INVERSE_MAX_CODE = 12`; probe-3 measured the set-wide worst over the twelve achromatic plus four skin patches at **13 / 7 / 4** for sizes 17 / 33 / 65, so **4 measured against 12 is a 3.0× margin** and draft v2's "≈ 4" inference is confirmed. Read as a *selection rule* the sweep would choose 33 at a 1.7× margin, so the contract pins the size and requires the sweep to be monotone non-increasing with size 17 genuinely failing. The black patch's **+4** is a property of the curve at every size; the failing direction is an identity 33³ cube at a set-wide **85** (7.1× over), and the gate is the **set-wide** worst because `chart06` alone reads 1 under that identity cube and a single-patch gate would be vacuous. The 65³ file measures **7 414 990 B**, 44.2 % of `LUT_MAX_FILE_BYTES`.

**No OPEN (drafter) notes remain.** Draft v1's OPEN-1 through OPEN-6 and draft v2's OPEN-T are all resolved by A1–A20 on measured evidence.

### 0.2 Changes from the critic

The skeptical critic reviewed draft v2 against the code at `99faee3` and returned **8 blockers, 19 majors, 13 minors, 22 confirmed and 7 open questions**; `rulings.md` decides every one and is binding. Draft v3 had already absorbed B3, B5 and B6 through A12–A22. The rest are folded here, one line each with its ruling id.

**Blockers.**

- **B1 → R-B1.** None of the eight `EvalAssertion` variants could be evaluated where draft v2 put them: `evaluate_assertion` sees only `EvalDefinition` and `EvalOutcome`, and `EvalOutcome` carries no `Analysis`, no `Core`, no exporter, no `original_document` and no per-call tool log. The measurements are now computed **inside `run_eval_with_artifacts`** and carried on `EvalOutcome` as a typed `color: Option<ColorEvalEvidence>` block plus `original_document`; `TrackLowConfidenceSamplesExactly` is **replaced by `TrackKeyframesMatchExpected`**, which reads the committed document's keyframes instead of a log that does not exist (§7.5).
- **B2 → R-B2.** `import_lut_asset` can never succeed in an eval run today, because the runner starts the server with a fresh `None` project path — so c3's prompt was unsatisfiable. `PreparedFixture` gains `project_path: Option<PathBuf>` and the runner calls `server.set_project_path`; stated as a **shared-runner change** affecting all six suites, with v1–v5 passing `None` (§7.6).
- **B3 → A21** (already folded): the log-signature constants were 8-bit against 16-bit fields.
- **B4 → R-B4.** `Matte::coverage` **multiplies** the window and qualifier legs, so draft v2's "(d2) = (d) plus a window" would have measured `192 / 140 / 52`, not the feather band. **(d2) is now a separate window-only node** with the qualifier off — two nodes, two canonical documents (§2.5, §4(d)(4)).
- **B5 → A15** (already folded): the spread budget of 6 passed the unmatched candidate; it is 5.
- **B6 → A12–A18** (already folded): every (f) gate was written against tracker behaviour that does not exist.
- **B7 → R-B7.** `EvalDeliverableSpec` has **no serde derives**, so draft v2's `#[serde(default)]` would not compile and its "v1–v5 specs omit it" was wrong — they are Rust struct literals. The field is plain and **every existing literal is edited** (~5 sites, named in §12 step 8); no `Default` impl (§7.6, §9.3).
- **B8 → R-B8.** The package was not blind: the reviewer had to open `human-review.json`, which names the task, and the leak test could not have caught it. §8 is rewritten around **`blind/review-form.json`** keyed on `blind_id` only, the key in the run root, `--score-review` resolving through it **before** `verify_review_artifact_bindings`, a leak test scanning the listing **and the form's bytes**, and an explicit statement that the **scenario identity is not blinded**.

**Majors.**

- **M1 → R-M1.** Crate attribution fixed throughout: `color_scopes.rs` / `color_status.rs` are **agent**, `matte_coverage_statistics` / `MatteCoverageStatistics` are **core** (`media.rs:673-726`).
- **M2 → R-M2.** Core has no path dependency on media, so `cc7_scenarios` carries its **own f64 transcriptions** of `encode_bt709` / `decode_display709` / `grade709_decode` with a named owner comment (CC6's `cc6_core.rs:35-37` precedent), cross-checked from the media side by the new fixture **`cc7_core_transcriptions_agree_with_the_pipeline`** (§2.7, §11.2.12b).
- **M3 → R-M3.** `EvalResult` is `Serialize`-only and `results.jsonl` is write-only, so `#[serde(default)]` was inert and the "pre-CC7 record parses" fixture could not be written. `measurements` uses **`skip_serializing_if` only**, and the fixture becomes `cc7_a_v5_result_serialises_byte_identically_without_measurements` (§7.6, §9.2, §11.2.32).
- **M4 → R-M4.** `bypass_matches_absent` is nested under `look_comparison` and is only ever `true` — a mismatch is the typed refusal. The failing direction is now **`bypass_not_lossless`**, with the hash-inequality check named as the fallback if no construction reaches it (§4(e)(1)).
- **M5 → R-M5.** `mean_hue_centidegrees` is `Option`: the gate requires `Some` on both sides and equality, and `in_band_basis_points` is stated as a rate over **considered (chromatic)** pixels with `excluded_achromatic_pixel_count` reported (§4(d)(3)).
- **M6 → R-M6.** `matte_coverage_statistics` takes one argument and has no ROI; the fixture **crops the coverage raster** first, and the field is `coverage_histogram` (§4(d)(1)).
- **M7 → R-M7.** `DeliveryVerification` has no `provenance` field — the claim is dropped — and `verify_delivery_output`'s trait default is `NotImplemented`, so the gate holds against `FfmpegMediaEngine` only (§4(g)(1)).
- **M8 → R-M8.** The canonical planner values are **regression pins** from an independent f64 transcription of `color_scopes.rs:1860-1965`, and the contract says so; refreshed to probe-2's B `+477 / −45 / +6` and C2 `+2 410 / +100 (raw +248) / −30`, with C1 measured at §12 step 5 (§2.5, §5.1(4)).
- **M9 → A16** (already folded); the per-node `+116` is **confirmed at step 5, never pinned from the report**.
- **M10 → R-M10.** The luma numbers are probe-2's, and **corrected C2 passes the luma gate** — (b2)'s failing direction is the spread gate alone (§4(a)(3), §4(b), §4.1, §4.2).
- **M11 → R-M11.** The `uses_outside_prose` needles **must be in an array literal**, normatively: a single-argument helper call self-matches, and the array form is why CC6 escapes (§11.3).
- **M12 → R-M12.** Step 6 had a compile-time dependency on steps 4, 7 and 8 through `CC7_TEST_SOURCES`. The inventory arrays and the both-direction assertion move to a **final step 9b**, and the dependency sentence is rewritten (§12).
- **M13 → R-M13.** There are **three** hard-coded `Eight` sites: `eval.rs:992` and `:1132` take the spec; `:6157` in `evaluate_caption_safe_area` is **named out of scope** (§7.6).
- **M14 → R-M14.** `deep_shadow` is in the linear segment, so all three decodes agree there and it cannot be a failing direction; the transcription fixture uses **`skin_light`** (§11.2.3), and §2.4.1's path text no longer implies `decode_display709`.
- **M15 → R-M15.** The manifest records **`raster.luma_percentiles` per scenario** (16-bit, from `measure_scopes`), which the roadmap's threshold paragraph requires and `analyze_color_shot` already returns (§11.3).
- **M16 → R-M16.** The two cut candidates were two matrix cells, so **(d2) and the (e) portability check are non-cuttable**. The cut order becomes the `EvalMeasurement` block, then the suite's `c4`–`c6` tasks, each with a written fallback and a §13 deferral (§12, §13).
- **M17 → R-M17.** `observations[]` and `low_confidence_samples[]` hold the same object shape; every frame assertion maps **`.local_frame`**, position reads **layer-space** `center_{x,y}_basis_points`, and the composite keys are separate (§5.2).
- **M18 → R-M18.** Restated: thresholds are variant **fields** in every existing variant too; CC7's rule is that the **suite call site** passes a `cc7_scenarios` constant instead of a literal (§7.5).
- **M19 → R-M19.** `plan_shot_match` takes **one shared ROI**; an absent `proposal_details` key means *not proposed*, so the (b1) gate iterates present controls **and** asserts `temperature_percent` is present; `reference_retained` is a hardcoded literal and is **not** cited as evidence (§4(a)(1), §4(b)(1)).

**Minors, all thirteen applied as ruled:** `CC7_LOOK_BLUE_ZERO_CROSSING_DISPLAY709_MILLIONTHS` renamed for its encoding (1); the percentile-convention citation corrected to `scopes.rs:1319-1338` (2); the `Rect`-branch and `feather <= 0` caveat on the distance field (3); the trivially-true `MATTE_TRACK_DEAD_ZONE_BASIS_POINTS` dropped from the distinctness list (4); `is_packaged_benchmark` cited at `:226-234` with four ids (5); `matte_window_row`'s full nine controls (6); the (e) app test placed in `look_browser_ui.rs`'s `mod tests` so its `CC7_TEST_SOURCES` entry is non-empty (7); §11.0.1's **source-content exemption** stated explicitly for `cc7_sources` (8); the baseline-test hazard restated as the exhaustive `matches!` in `published_v5_…` (9); the three >10× 10-bit rows explained as CC6 constants CC7 cannot move (10); the §11.0 citation range verified as `:1352-1362` (11); "reuses `EvalBudgets`' field set", not `standard_budget`'s values (12); `max_turns` cited at `eval.rs:878` with the scored-vs-enforced distinction (13).

**Open questions.** Q1 (C1 unmeasured) → §12 step 5, implementer, media fixture. Q2 → closed by probe-3. Q3 → `write_review_package` writes `blind/` and `blind-key.json` for **every** packaged run, no flag, `print_usage` unchanged (§8.2). Q4 → `ground-truth.json` is generated by `cc7_scenarios::ground_truth_json()` and the manifest test asserts byte equality; **no environment knob** — the test prints the diff and the implementer pastes (§7.7). Q5 → the negative statement that **no CC7 fixture asserts an adapter string** (§14). Q6 → the whole CC7 media suite is budgeted at **180 s** on Linux, with CC6's measured totals quoted for comparison (§14). Q7 → recorded with R-M9.

### 0.3 Changes from implementation and review

Seven implementers built the slice — **B** (the shared eval runner), **A** (`cc7_scenarios`, `cc7_sources` and §11.2 items 1–11), **E** (the seven §6 person-path tests), **F** (the `color-workflow-v6` suite and `benchmarks/auto-edit/v6/`), **D** (the six §5.2 agent scripts), **C** (the media fixtures, the §12 step 5 measurements and `cc7_manifest.json`) and **G** (step 9b's inventory) — and three first-pass reviews then read the result with fresh eyes: **R1** (eval harness, 1 blocker / 5 majors), **R2** (agent and app, 0 blockers / 4 majors), **R3** (`cc7_sources`, 0 blockers / 4 majors), and **R4** (core authority, media gates and manifest, 1 blocker / 2 majors / 11 minors); fixers **X** (R1), **Y** (R2), **Z** (R3) and **W** (R4 plus step 9b's final inventory) then applied the rulings, each verifying its predecessor's work on disk before finishing it. `target/review/cc7/errata-from-implementation.md` is the record, append-only, one section per implementer; `rulings.md` decides every escalation and every review finding and is binding. Every item below is either a change to something this contract stated — an API shape, a stored parameter set, a constant value, a test name, a gate, a file placement — or a measurement §12 step 5 asked to be taken; no CC6 budget was widened and no existing test was renamed, deleted or weakened.

**Contract changes.**

- **B-E1.** `EvalDefinition` gains a plain `color: Option<ColorEvalRequest>` field beside `deliverable`, edited into all twelve definition literals in `kinewright-eval.rs` and all four in `eval.rs`'s tests as `color: None`; `ColorEvalRequest::from_assertions(&[EvalAssertion])` derives the request and returns `None` when no colour assertion is present. §12 step 8 named the evidence block but never said where the *request* — the ROIs, the matte region, the measured depth — comes from.
- **B-E2.** `ColorEvalEvidence` carries **ten** fields, not §7.5's nine: a tenth, `errors: Vec<String>`, records measurement failures so an assertion arm can tell "not asked for" from "asked for and unmeasurable". `color_not_measured` puts the recorded reason into the detail, and `cc7_delivery_verification_without_a_deliverable_is_recorded_as_an_error` asserts both directions.
- **B-E3.** `measure_color_evidence(&ColorEvalRequest, &dyn Analysis, &Arc<Document>, Option<(EvalDeliverableSpec, &EvalDeliverableResult)>) -> ColorEvalEvidence` does **not** take `&dyn Export`; was R-B1's "where `fixture.analysis` / `fixture.exporter` / `core` / `original_document` are all alive". `Analysis::verify_delivery_output` reads the file the exporter already wrote and re-renders its own reference, so the parameter would have to be `_exporter` on a public signature.
- **B-E4.** `verify_delivery_output` runs inside `measure_color_evidence`, immediately after the deliverable step and before the timeline is restored, gated on `ColorEvalRequest::delivery_verification`; was §4(g)(1)'s "in the deliverable path when the spec asks". `produce_deliverable` is shared with `render_saved_deliverable` and every v1–v5 task and has no colour request in scope.
- **B-E5.** `PreparedFixture::new(document, media, context, project_path, resources)` gains a parameter rather than a builder, and all ten existing call sites pass `None`; §9.6's "v1–v5 pass `project_path: None` explicitly" is the reading that makes the sentence true, and it matches R-B7's reason for refusing a `Default` on `EvalDeliverableSpec`.
- **B-E6.** `human_review_template(benchmark_id, run_id, results)` keeps its signature and delegates to `human_review_template_with_questions(…, &questions)`, where `questions: &BTreeMap<String, Vec<HumanQuestion>>` is keyed by **base** task id; was §8.3's "gains, for the colour benchmark id only, `blind_id`, `questions` and the pre-marked `not_applicable`". `write_review_package` calls the four-argument form through `review_questions(benchmark_id)`, the marked registration point.
- **B-E7.** §4's two-pixel patch inset is applied to the eval block too, so `neutral_spread_max_code` and `chart_luma_mean_delta_millionths` over one rectangle are the same measurement in both lanes; a rectangle too small to inset is measured whole, and `MatteContainmentExact`'s coverage crop is **not** inset because those are exact counts.
- **B-E8.** Adding required fields forces every `EvalDeliverableSpec` / `EvalDefinition` / `EvalOutcome` / `HumanTaskReview` literal to name them, including literals inside existing tests: `delivery_bit_depth: DeliveryEncodeDepth::Eight`, `color: None`, `original_document: Document::default()`, `blind_id: None`, `questions: Vec::new()`. One existing case, `fake_driver_eval_accepts_the_transcript_clamped_bound_and_rounding_allowance`, crossed clippy's 100-line limit by one line and carries a targeted `#[allow(clippy::too_many_lines)]`; its body is otherwise untouched.
- **B-E9 → F-E9.** B landed `CC7_LEAK_NEEDLES` / `CC7_LEAK_VALUE_NEEDLES` as local literal arrays because `cc7_scenarios` was being written in the same slice; F replaced both with derivations and the erratum is closed.
- **A-E1.** §2.2's `const fn` declarations do not hold: the erratum's heading says three and its "Done" says "all four are plain `pub fn`" — `cc7_analytic_square_top_left`, `cc7_analytic_square_centre_basis_points`, `cc7_log_encode_code` and `cc7_log_inverse_display`. `f64::sin`, `f64::log2` and `f64::powf` are not callable in a `const fn` on stable Rust 1.92; `cc7_tracking_sample_frames`, `cc7_spec` and `cc7_camera_transform` are `const fn` as stated.
- **A-E2.** §2.2 gains `cc7_b1_canonical_operations()`, `cc7_d2_canonical_operations()` and `cc7_track_keyframe_operations()` (over `CC7_B1_OPERATIONS`, `CC7_D2_OPERATIONS`, `CC7_F_KEYFRAMED_PARAMETERS`); `cc7_canonical_operations(scenario)` alone could not reach the seven documents §2.5 defines over six scenarios. `Cc7Scenario::WhiteBalance`'s spec carries (b2)'s document, because §5.2's `cc7_b_…` script commits C2.
- **A-E3.** `CC7_D2_OPERATIONS` carries **eight** parameters and not `matte_window0_shape_token`; was §2.5's (d2) row listing `matte_window0_shape_token = 1`. The rect shape token's descriptor neutral **is** `1` (`effect.rs:744`) and both writers filter neutrals, so storing it would make the canonical document differ from every real commit — the same rule §2.5 already applies to (c)'s `input_encoding_token = 0` and (e)'s `mix_basis_points = 10_000`.
- **A-E4.** `CC7_F_OPERATIONS` carries **five** parameters — `matte_enabled`, `matte_window_count`, the two half extents and `saturation_percent` — and not `matte_window0_center_x_basis_points = 5000` / `_center_y_basis_points = 5000`; those are the descriptor neutral (`effect.rs:749-761`), are exactly frame 0's square centre (§2.3.6), and arrive only as the `SetEffectKeyframes` curves.
- **A-E5.** (d)'s node stores `matte_enabled`, `matte_qualifier_enabled = 1`, the nine derived bands and `saturation_percent` — **twelve** entries, a qualifier group of ten; was §2.5's "`matte_enabled = 1` and the nine `matte_*` qualifier parameters". `derived_qualifier` produces nine bands (`color_status.rs:4970-5014`) and `matte_request_parameters` injects the switch separately (`:4533-4544`), whose neutral is `0`.
- **A-E6.** `cc7_canonical_operations` returns the node operation alone for (c) and (e), and `cc7_lut_backed_canonical_operations(scenario, asset)` prepends the `AddLutAsset`, built by `cc7_log_lut_asset(sha256, byte_len, source_path)`; was §2.5's single batch. §2.1 forbids core from reading a file, and a fabricated digest would fail §5.1(4) against every real import.
- **A-E7.** `#[cfg(any(test, feature = "test-util"))] pub mod cc7_sources;`; was §3's "**Public, not `cfg(test)`**, in the shape of `test_support`". `test_support` is itself gated exactly that way at `lib.rs:26-27`, and `cc7_sources` returns `test_support::GeneratedMedia`, so it cannot compile outside the gate — the intent (visible across a crate boundary) is met.
- **A-E8.** A11's "the module doc says so in the same words `test_support`'s does" is not satisfiable: `test_support.rs` opens on a `use` and has no module doc. The words are taken from `run_ffmpeg`'s own `# Panics` section, citing `test_support.rs:274-297`.
- **A-E9.** §2.6's distinctness test asserts distinctness **within a unit** (18 same-unit pairs, asserted non-vacuous) and states the one cross-unit coincidence rather than asserting it away: `CC7_FEATHER_PARTIAL_TOLERANCE_PIXELS` is `4` **pixels** and `DELIVERY_CODEC_MAX` is `4` **8-bit codes**. Neither constant can move (A7 pinned one, CC1/CC6 own the other), and a pixel count cannot be silently substituted for a code tolerance.
- **A-E10.** §2.6's neighbouring constants are restated in `cc7_core.rs` as `const`s with owner/file/line comments, exactly as R-M2 does for the transfer functions, because they live in `kinewright-media` and `kinewright-agent` and core depends on neither. `MONITOR_CPU_GPU_P99` / `_MEAN` and `DELIVERY_CODEC_P99` / `_MEAN` leave the list: they are `f64` and §2.6 states no CC7 constant is a float.
- **A-E11.** §11.2.7's failing direction is stated per axis: 130 px of **y** amplitude leaves the raster (`78 + 130 + 24 = 232 > 180`), 130 px of **x** still fits, and 149 px of x leaves it. On the x axis the square only leaves at amplitude > 148, so the contract's bare "130 px" would have been vacuous there.
- **A-E12.** §4(d)(4)'s `76.8` is the **nominal** 6 × 8 px window; the basis-point-quantized `5.984 × 7.992` half-extents the same paragraph states give `76.5`. The fixture asserts both, and that neither is within `CC7_FEATHER_PARTIAL_TOLERANCE_PIXELS` of the measured 112; `CC7_D2_CONTINUOUS_AREA_WRONG_MODEL_PIXELS_TENTHS = 768` is the nominal figure.
- **A-E13.** §2.4.2's wrong path differs from the correct one on **green only** at `skin_light` (`[160, 146, 139]` against `[160, 147, 139]`), so the fixture compares the whole triple and asserts the red channel agrees; `deep_shadow` (`[33, 33, 33]` wrong against `[32, 32, 32]`) is asserted beside it. A single-channel assertion at `skin_light[0]` would have been vacuous.
- **A-E14.** §3.5(1)'s "each of the **twelve** achromatic chart patches differs from cam A's code" cannot hold: the fixture asserts the **eleven** patches that carry light differ and that `chart00` is **identical** under B, C1 and C2. The black patch decodes to exactly zero scene-linear light and no gain, exposure scale or luma-preserving saturation mix moves zero; the whole-raster mean absolute differences (B 20, C1 52, C2 76) are unaffected.
- **A-E15.** A17's 746 bp final-keyframe lag is measured against the **analytic** centre (`7 246 − 6 500`), not the observation (`7 246 − 6 465 = 781`); recorded so a downstream gate does not compare the keyframe against `observations[]` and expect 746.
- **A-E16.** A12's "the step clamp is reached exactly once" is a claim about **raw** segments: exactly one raw segment exceeds `MATTE_TRACK_MAX_STEP_BASIS_POINTS = 800` (the `4 → 9` x segment, 898), while the smoothed series carries **two** differences equal to 800 as the curve catches up. The fixture asserts the raw count, the smoothed 800 on that segment, and a worst smoothed-to-raw residual ≤ 98 bp.
- **A-E17.** §11.2.3 compares §2.4.1's printed linear and display columns at `5e-7`, a half-ulp of the table's own six printed decimals; was "within `SPEC_F64_TOLERANCE`" (`1e-6`). `SPEC_F64_TOLERANCE` is unchanged and still governs every full-precision `f64`-to-`f64` comparison, including the 1 001-point sweep of all three transfers.
- **A-E19.** §3.4's `.cube` header is CC4 §2.6's pinned canonical text (`lut.rs:219-240`) — a `TITLE` line, `LUT_3D_SIZE`, and two **six-decimal** domain lines — not the stated `DOMAIN_MIN 0 0 0` form, which is 49 bytes and yields `7 414 924`. Probe-3's three sizes solve to a 115-byte header over 27-byte sample lines, exactly `100 + title.len()`; `CC7_CUBE_TITLE.len() == 15` is asserted because `CC7_LOG_CUBE_BYTES_REPORTED = 7 414 990` depends on it.
- **A-E20.** `cc7_scenario_sources(WhiteBalance)` returns **three** rasters — cam A, C1 and C2 — because (b1) and (b2) are two documents over one reference; §3.1 declared the function without saying what (b) returns. `cc7_scenario_source_kinds(scenario)` exposes the same list without shelling out to FFmpeg.
- **A-E21.** Three helper items beyond §2.2's and §3.1's lists are named so a downstream implementer does not re-declare them: `cc7_scenarios::cc7_stabilized_centres` (cross-checked both directions by `cc7_the_keyframe_smoother_transcription_matches_core`), `cc7_sources::identity_cube` / `write_identity_cube`, and `cc7_sources::cc7_source_frame_planes` / `cc7_source_planes` / `cc7_bt709_limited_source_codes`.
- **E-E1.** §12 step 7's verification command is `cargo test -p kinewright-app --bins -- cc7_`; was `--lib`, which fails with "no library targets found in package 'kinewright-app'" because the crate is a binary.
- **E-E2.** §6's batches for (a), (b), (d) and (d2) are applied to the base document seeded with the canonical batch passed through a local `cc7_unvalued`, which rewrites every `InsertEffect` to carry an empty parameter map; (c) and (e) apply to the bare base document because their builders create the node. `add_effect_operation` stores every non-matte descriptor parameter at its neutral (`inspector_ui.rs:4118`), so a node built the app's way can never equal the canonical document without the strip.
- **E-E3 → R2-MAJ-4.** §6's headless pattern cannot *move* an `egui::Slider`: each test renders the real section (`primary_correction_section`, `matte_section_body`, `matte_window_row`, `look_mix_row`), asserts drawing writes nothing, asserts the painted control names through `crate::theme::painted_text`, and then builds the batch through the exact call the widget's `changed()` branch makes. The one live-widget measurement was (b)'s bound — C2's raw `+248` clamped to `+100` — through a slider the test replicates from `EFFECT_DESCRIPTORS`; the R2-MAJ-4 fix has since added the **card's** own bound as a second live measurement and the erratum is corrected (Y-E4).
- **E-E4.** `matte_window_row` draws **ten** items — eight value controls (`MATTE_WINDOW_PARAMETER_COUNT = 8`) plus the "Select in viewer" and "Remove" actions; was §6's "nine controls", a sentence that lists eight controls and two actions.
- **E-E5.** The (d2) test asserts `matte_window0_shape_token`, `_rotation_centidegrees` and `_invert` are **absent** from `CC7_D2_OPERATIONS`, and that `matte_add_window_operations` on a matte-free node emits exactly `{matte_enabled: 1, matte_window_count: 1}`, because a fresh window **is** `MatteWindowParams::NEUTRAL`. This is A-E3 seen from the app side: the descriptor-neutral rule is a property of both writers, not of the planner alone.
- **E-E6.** `inspector_ui::look_mix_row` and `InspectorEdits::operations` become `pub(crate)`; §11.2.31 puts the (e) test in `look_browser_ui.rs` while §6 names a builder in `inspector_ui.rs`. No other visibility moved and no non-test code changed.
- **E-E7.** (c)'s app test supplies a syntactically valid placeholder digest to both `LutAssetImport` and `cc7_log_lut_asset` and asserts the two records are equal field for field; was §6's implicit hashing of the real `.cube`. The generator is behind `kinewright-media/test-util`, which `kinewright-app` does not enable (A-E6); the digest itself is pinned by the media and agent fixtures, and the convention already exists at `media_workflow.rs:2639-2652`.
- **E-E8.** The unnamed seventh §6 test is `cc7_d2_a_person_can_author_the_window_only_matte_by_hand`, so step 9b's `CC7_APP_TESTS` has a name to resolve.
- **E-E9.** `cc7_b_the_temperature_slider_stops_where_the_planner_clamps` authors and applies **both** (b) documents and asserts they differ; §6's table has one (b) row and §11.2.31 counts seven app tests, so (b1) cannot have its own test and would otherwise have no person-path evidence at all.
- **F-E1.** Filling `review_questions` with the six scenarios' `human_question` entries keyed `c1`..`c6` — **`c3` contributing no entry at all** — makes §8.3's "when `accepted` is `Some(_)`, every question must be answered" rule bite on three existing CC7 tests, which each gained an answer step; no test was renamed, deleted or weakened.
- **F-E2.** `color_workflow_budget(max_tool_calls: u32) -> EvalBudgets`; was §7.4's `usize`, which `EvalBudgets::max_tool_calls` would have to narrow at the one call site. `c6` spreads over it in `speech_budget`'s existing shape, and `max_undos` — which §7.4 never names — is set equal to `max_operations` at every task.
- **F-E3.** `c4` passes `CC7_MATTE_OUTSIDE_CHANGED_PIXELS_MAX` (`= 0`) into `expected_partial_pixel_count`, because R-M18 forbids the suite call site writing §4(d)(1)'s literal zero; `cc7_every_color_assertion_threshold_is_a_cc7_scenarios_constant` asserts the identity.
- **F-E4.** `c4` and `c6` leave `matte_node: None` for the runner's "the one matte-enabled node" resolver rather than pinning an `EffectId` the model did not choose; `c5` pins `CC7_SINGLE_CLIP_ID` / `CC7_NODE_EFFECT_ID` because `LookBypassMatchesAbsent` has no resolver.
- **F-E5.** `write_log_like_inverse_cube(dir, CC7_LOG_CUBE_SIZE)` writes `cc7-log-inverse-65.cube` — the generator names the file after its lattice size so the §4(c)(3) sweep can write three cubes into one directory — and `fixture_cc7_log_like()` renames it to the prompt's `log-inverse.cube`, asserted on disk by `cc7_every_color_fixture_builds_a_valid_document`.
- **F-E7.** All six specs are `DeliveryEncodeDepth::Eight` per A10, and every `DeliveryVerificationWithinBudgets` asserts the depth its task encodes so a task cannot verify a lane it did not write; `eval_suite` accepts `"color-workflow-v6" | "v6"`, with the long form in the unknown-suite error.
- **F-E8.** `print_usage`'s banner moves into `fn usage_text() -> &'static str` so `cc7_the_usage_banner_lists_every_registered_suite` can read it; a `println!` cannot be asserted without capturing stdout, and "registered but undiscoverable" is what §7.1(4) exists to prevent.
- **F-E9.** `cc7_leak_needles()` derives the task ids from `color_workflow_suite()` and every parameter name from the canonical operations, and `cc7_leak_value_needles()` derives six values from `CC7_MATCH_PROPOSAL_{B,C1,C2}.exposure_milli_stops`, `CC7_LOOK_MIX_BASIS_POINTS`, `CC7_TRACK_MIN_CONFIDENCE_BASIS_POINTS` and `CC7_PRODUCT_SAMPLE_HUE_MEDIAN_CENTIDEGREES`; was §8.5's "any canonical parameter value". The narrowing is normative and asserted — every value needle is at least three digits and the set has six members — because a two-digit needle matches a digit pair anywhere in a package; `input_encoding_token` and `mix_basis_points` stay literal, being the two neutrals §2.5 deliberately does not store.
- **F-E10 → R1-M2.** `c1` and `c2` set `chart_luma_roi = CC7_CHART_BAND_ROI` and `c5` sets `gamut_roi = CC7_DEEP_SHADOW_ROI` as ungated evidence, asserted exhaustively by `cc7_every_color_task_carries_a_color_eval_request`; F-E10's claim that both quantities "reach `results.jsonl`" was false and R1-M2 corrects it.
- **F-E11.** The Q4 generator is `cc7_ground_truth_json()` in `kinewright-eval.rs`'s test module, not `cc7_scenarios::ground_truth_json()`: §2.1 makes core "data and arithmetic only" and a JSON writer is neither. Everything it emits still comes from `cc7_scenarios`, and `published_v6_manifest_tracks_the_color_workflow_suite` prints the generated bytes on a mismatch with no environment knob (§10).
- **D-E1.** The CC1 primary planner publishes `AddEffect` with **ten** non-matte controls at their descriptor neutrals plus `SetEffectParam ×N` (`color_status.rs:1529-1540`, `:1598-1610`); was §2.5's "one `InsertEffect` … carrying `exposure_milli_stops`, `temperature_percent`, `tint_percent`; no `saturation_percent`". §5.1(4)'s equality is taken with those neutrals filled in by `cc7_with_cc1_neutral_fill`, which reads the neutral set from `effect_descriptor("primary_correction")` and `is_matte_parameter`; (d) and (f) are unaffected and match exactly with no fill.
- **D-E2.** §5.1(2)'s typed `stale_revision` is published only by `analyze_color_shot`, `plan_shot_match` and `get_color_qc`; `plan_technical_lut`, `plan_creative_look`, `plan_secondary_correction` and `track_matte_window` route through `revision_conflict_text` (`server.rs:13255-13259`), which is prose with no `structured_content` and no `code`. Two helpers split the claim, and §5.1(2) is owed a narrowing — CC7 must not move those planners onto the typed envelope, which would be a behaviour change to a shipped surface.
- **D-E3.** (c)'s `get_color_qc` and `render_color_proof` are asserted as **unconditional typed refusals** (`working_proof_unavailable` naming `missing_lut_asset`, and the typed `missing_lut_asset` with `lut_asset_id`, `effect_id`, `lut_sha256`, `stage: "after"`) and are **not** wrapped in §5.3's GPU-unavailable branch: the agent server never publishes an imported asset's bytes to the renderer, so the refusal is deterministic and a skip branch would assert nothing.
- **D-E4.** `SecondaryCorrectionPlanArgs` has no colour control at all (`color_status.rs:4326-4374`), so §5.2's single (d) call with `saturation_percent: 40` is two planner calls and two commits — the matte through `plan_secondary_correction`, then `saturation_percent` through `plan_primary_correction`, which retargets the same node in place and emits `SetEffectParam` alone. The revision advances exactly once per commit and the final documents equal the canonical batches exactly.
- **D-E5 → C-E3.** C1's proposal on the amended scene is `exposure_milli_stops +1 465` (unrounded 1 464.538582207817) `/ temperature_percent +81` (unrounded 80.81971558262846) `/ tint not proposed`; was `+1 432 / +77 / −3`. `CC7_MATCH_PROPOSAL_C1` is re-pinned and `CC7_B1_OPERATIONS` stores **two** parameters, not three, because C1's tint delta rounds to `0` and `color_scopes.rs:1897-1903` omits the key — the one place R-M19's absent-key rule is exercised by a real measurement rather than by construction.
- **D-E6.** The `matte_window_index_out_of_range` refusal is asserted in **(f)** at `window_index: 4`, where §5.2's (f) row already places it: `InspectGradeMatteArgs` (`server.rs:8669-8684`) has no `window_index` and ignores the extra key. (d) asserts `matte_unsupported_node_kind` and `color_qc_region_required` instead.
- **D-E8.** QC exception `severity` serializes lower case on the wire (`"warning"`), and `observed` is a **string**, not a number; the contract's tables and prose spell the severities `Warning`, `Info`, `Error`. Recorded so no later fixture compares the capitalised spelling.
- **D-E9 → R2-MAJ-3.** §5.4's registry `1 280 060 B` is not reachable from an integration test — `capability_tools()` and `served_tools()` are private and `ToolSurfaceMetrics::measure` needs `Vec<Tool>` — so `cc7_the_agent_surface_is_unchanged_by_this_slice` asserts the public, byte-exact quantities instead (7 served names, `capability_tool_names().len() == 124`, `124 − operation_tools().len() == 75`, and `{5 660, 3 510, 998}`). D-E9's claim that the byte count "stays pinned" elsewhere was false when it was written; the R2-MAJ-3 fix has since landed and the erratum is corrected (Y-E3).
- **D-E10.** `track_matte_window` publishes `prepared_edit_plan.{plan_id, expected_revision, preview}` (`server.rs:4700-4712`) and no top-level `operations`, so (f) commits **that** plan id rather than re-preparing; §5.2's (f) row reads "→ prepare/commit" like every other row.
- **D-E13.** `cc7_f2_the_default_floor_does_not_refuse` is a **standalone** eighth agent test, as §11.2.28's sub-bullet names it, in addition to the same two directions asserted inside `cc7_f_tracked_secondary_drops_only_the_occluded_samples`; `CC7_AGENT_TESTS` therefore has eight entries, not §12 step 4's seven.
- **C-E1.** §11.2.13's A1 guard and §3.5(7) are the same fixture under two numbers: `cc7_the_chart_band_is_achromatic_and_the_primaries_band_has_no_red` lives in `cc7_sources.rs` (Implementer A) and `cc7_fixtures.rs` cites it in the manifest's `raster.a1_guard.asserted_by` rather than re-declaring it.
- **C-E2.** `assert_matte_containment` and `ContainmentCounts` (with its six fields) become `pub(crate)` in `cc5_fixtures.rs`, the minimal edit that makes §4(d)(2)'s "reused rather than restated" expressible; nothing else in that file changed.
- **C-E11.** `cc7_log_lut_asset`'s record pins `CC7_LOG_CUBE_SIZE`, so the imported-`LutAsset` equality applies at size **65 only**; the §4(c)(3) sweep imports 17 and 33 through the same helper and the record's `size` field stays 65. The constraint was found when the equality failed on the sweep's size-17 rung.
- **C-E12.** §11.2.12b's seam probes are offset by `±1e-6`, not by an `f64` hair: `color_pipeline`'s functions are `f32`, whose ULP at `0.018` is `1.9e-9`, and a smaller offset measures the seam's own representation rather than the transcription (the first run failed at `0.018 − 1e-9` with `2.48e-4`). Over **49 515** comparisons the worst error is **8.57e-7**, inside `CC7_SPEC_F64_TOLERANCE = 1e-6`.
- **C-E13.** §4(a)(5)'s negative control perturbs the three colour channels only: `linear_parity_metrics` asserts the alpha sample never moves (`cc1_fixtures.rs:860`), so a perturbed alpha trips that assertion instead of the gate under test (the first run failed with "production alpha changed: actual=1 expected=1.003").
- **C-E16.** The manifest's `review.leak_test_needles` records the **resolved** sets — `task_ids`, `machine_provenance` (B's seven literals verbatim), `canonical_parameter_names` (derived and asserted equal to the media crate's own derivation) and `value_needles` — because B landed functions rather than the two arrays §11.3 names; a byte-equality assertion against `cc7_leak_needles()` belongs to step 9b, since the media crate cannot see the eval binary.
- **G-E1 → C-E7.** `CC7_B1_RESIDUAL_SPREAD_MAX_CODE: i64 = 6` is added to `cc7_scenarios.rs`, the (b1) row of `CC7_BUDGETS` names it, `CC7_MEASURED_B1_RESIDUAL_SPREAD_CODE` is re-pinned from **2** to **3**, and `CC7_MEASURED_UNCORRECTED_C1_SPREAD_CODE = 7` is new; the manifest gains threshold key `cc7_b1_residual_spread_max_code`, raising the asserted key count from 83 to **84**. See the escalation below for the ruling.
- **G-E2.** One `CC7_MEDIA_TESTS: [&str; 41]` covers `cc7_fixtures.rs` and `cc7_sources.rs`, with `CC7_MEDIA_TEST_SOURCES: [&str; 2]` naming them — CC6's shape at `cc6_fixtures.rs:2292` — rather than a separate `CC7_SOURCE_TESTS`; §11.3's array list names nine arrays and no such array.
- **G-E3.** Two §11.2 names drift from the tree and the inventory follows the tree: §11.2.32's `cc7_color_evidence_is_computed_where_the_analysis_is_alive` landed as **`cc7_color_evidence_is_measured_where_the_analysis_is_alive`** (a rename only; R1-M4 rules on it), and **`cc7_a_fixture_project_path_reaches_the_server`** does not exist under any name (R1-B1). The implementer brief's "15 in `eval.rs` / 12 in `kinewright-eval.rs`" measure **14** and **13**, the thirteenth being the non-prefixed `published_v6_manifest_tracks_the_color_workflow_suite`; the total, 27, is unchanged.
- **G-E4.** `CC7_FORBIDDEN_HELPERS` is `[&str; 3]`, the third needle being `"std::env::var"`; was §11.3's normative `[&str; 2]`. The array-literal form R-M11 requires is kept exactly, and every needle-derived string is assembled at run time from the array (`format!("{}(", CC7_FORBIDDEN_HELPERS[2])`), because writing the reader as a literal would make the guard match its own file.
- **G-E5.** §11.3's guard runs **per CC7 test body** in `tests/mcp_server.rs`, not per file: the file is shared with CC1–CC6 and legitimately consults the §5.3 opt-in in nine older places. `cc7_test_body(source, name)` slices each top-level `#[tokio::test]`, and the guard asserts `fixture_gpu_or_skip` absent, every `std::env::var(` being the opt-in read, and a `SKIPPED:` line plus one of the three typed codes in any body that reads it — **7 template branches** across five tests, with a `>= 5` floor so it cannot go vacuous.
- **G-E6.** The manifest's `required_fixtures` grows from **50** to **95** entries, one per declared test (`30a`–`30h` agent, `31a`–`31g` app, `32a`–`32aa` eval, `33b`, plus `8b` and `18b` that step 6 missed), and the manifest test asserts the exact equality `required_fixtures.len() == 95 ==` the deduplicated inventory, replacing step 6's `>= 29`. `inventory.status = "owed_at_step_9b"` is replaced by the real block, each field asserted equal to its Rust constant.

**Measured confirmations.**

- **D-E11.** Every remaining (a)/(b2)/(c)/(d)/(e)/(f)/(f2) gate reproduced the contract live on the first run, on the default lavapipe lane: `CC7_MATCH_PROPOSAL_B` `+477 / −45 / +6` unclamped; `CC7_MATCH_PROPOSAL_C2` `+2 410 / +100 (raw +248) / −30` with `min −100 / max 100`; the carrier's 16-bit luma percentiles `7 196 / 42 919` inside `5 140` / `51 400`; (d) `192 / 192 / 0` with `covered_basis_points 33`; (f)'s sample frames `[0,4,9,14,18,23,28,32,37,42,47]`, low-confidence set `{47}`, occluded confidence 7 349 and (f2)'s 7 309; and §5.4's `7 / 124 / 75 / 5 660 / 3 510 / 998`.
- **C-E3 (b1) proposal.** `+1 465 / +81 / tint absent`, both present controls unclamped with `requested == value`; the media replica reproduces `CC7_MATCH_PROPOSAL_C1` and `CC7_B1_OPERATIONS` byte for byte.
- **C-E7 (b1) residual.** **3** codes, worst patch `chart09`, against `CC7_B1_RESIDUAL_SPREAD_MAX_CODE = 6` — a **2.0×** margin; uncorrected C1 measures 7 and corrected C2 measures 19, so the gate is not vacuous in either direction.
- **A-E18 (c) floor.** Cam A's **authored**-raster luma p1 is 16-bit **2 827** (8-bit 11), not the decoded `2 570` (8-bit 10) `CC7_CAM_A_LUMA_PERCENTILES_CODE16` keeps; the one-code difference is exactly §2.4.1's "the decode round trip costs at most one code on a flat patch", both values fail the `5 140` floor, and the carrier's authored percentiles are identical to probe-3's decoded ones.
- **C-E4 (c) lattice sweep.** **14 / 7 / 4** through §4(c)(2)'s mandated `Analysis::monitor_proof_for_document`, against `CC7_LOG_CUBE_SIZE_LADDER`'s `13 / 7 / 4`; sizes 33 and 65 are asserted exactly, size 17 is asserted `>= 13` **and** `> CC7_LOG_INVERSE_MAX_CODE = 12`, with monotonicity and `17 > 12 >= 33 > 65`. The 65-rung's **4 against 12** is A22's 3.0× margin unchanged, and the manifest records both columns.
- **C-E14 (b2) per-node delta.** `range_basis_points_delta = +116` confirmed, never pinned (Q7): with the node `clamped_basis_points = 116`, without it `0`, `gamut_basis_points_delta 0`, `blue.over_pixel_count 672`, `blue.maximum_over_excursion_millionths 41 538`, red and green `0`, `technical_pass = true`, and exactly one `delivery_range_excursion` Warning on `blue.over_basis_points` observed `116` allowed `< 10`. Both control candidates measure `0 / 0` with no exception.
- **C-E15 (a) skin.** `in_band_basis_points = 10 000` on both cam A and matched cam B, `considered_pixel_count 768`, `excluded_achromatic_pixel_count 0`, `mean_hue` `12 325` reference against `12 243` matched; the skin-row chroma spread is **60.50** reference against **52.00** matched (was "52.75 against 60.50"), with the unmatched candidate at 50.25 — which is why §4(a)(4) is stated as "matched < reference" and not as a bound.
- **C-E9 / C-E10 / D-E7 rates.** Corrected C2's skin band reports `in_band_basis_points = 10 000` (was `CC7_C2_SKIN_IN_BAND_REPORTED_BASIS_POINTS = 9 411`), comfortably above `SKIN_BAND_EXCEPTION_BASIS_POINTS = 5 000`, and the `deep_shadow` ROI's `out_of_gamut_basis_points` is **10 000** (was 9 411) both in the media fixture and live on the endpoint. Both 9 411s are `192 / 204`, the rate under the naive pre-A19 `y_basis_points = 4222` ROI; A19's `4223` resolves to exactly 192 pixels, the gated counts are unaffected, and both constants are reported-never-gated.
- **C-E6 (f) containment.** Required half-extents **14.784 / 12.882 px** against the 1.5× window's **18.016 / 18.000 px** — margins **3.232 / 5.118 px**, within two hundredths of a pixel of the pinned `1 477` / `512`; the seeded 1.0× window is **2.78 px** short in x against the contract's 2.77. The gate remains containment itself at every surviving sample except the named final keyframe 42, which is asserted to genuinely fail.
- **C-E5 (e) failing direction.** The look-free base scene reports `out_of_gamut_pixel_count == 0` on both the ROI and the whole raster as stated, but raises two `delivery_range_excursion` Warnings on the **whole raster** — blue over `44` bp (256 px), green over `22` bp (128 px) — from the primaries band's saturated channels under the limited→full decode. The fixture gates "no `delivery_gamut_excursion`", which is the Warning §4(e)(2) means, and asserts the `deep_shadow` ROI raises no exception at all.
- **C-E17 (g) failing direction.** The (a) canonical document starved at `-b:v 100k`, 8-bit: `within_budgets = false`, `technical_pass = false`, one `decoded_difference_over_budget` **Error** on `luma.maximum_code_diff` observed `17` allowed `<= 8`, with `luma_p99 2 000 000`, `luma_mean 107 549`, `rgb_mean 427 720`, `psnr 4 176`; the output file is left at its original path, unrenamed and undeleted.
- **D-E12 / G-E7, rule 11.0.5.** Every agent gate was perturbed once, rebuilt, and observed to fail — including both §5.1 shared helpers, both stale-revision forms, and every typed refusal — and three (D-E1, D-E5, D-E8) were found failing organically before they were fixed; the step 9b inventory test was likewise observed failing in both directions, once by deleting a media test name and once by misspelling an eval one.

**Escalations and their rulings.**

- **C-E7 → G-E1 (ruled, landed).** The (b1) residual's **1.67×** margin (3 measured against the shared `CC7_MATCH_NEUTRAL_SPREAD_MAX_CODE = 5`) breaks §4.1's `budget / measured ≥ 2` rule, and §14's re-baseline fallback cannot apply because the constant is shared with (a); the budget cannot widen to 6 either, because 6 is exactly what *unmatched* cam B measures and A15 excludes it. The ruling splits the rows: `CC7_B1_RESIDUAL_SPREAD_MAX_CODE = 6`, deliberately one code above the (a) budget and asserted so, with (a) untouched at 5 — defensible on the scenarios' own terms, since (a) matches a candidate against a reference on the same chart while (b1) recovers a clip that arrives wrong-balanced **and** underexposed from a proposal alone. Measured: corrected C1 **3** (2.0×, passes), uncorrected C1 **7** (fails), corrected C2 **19** (fails), with `cc7_b1_the_uncorrected_candidate_exceeds_the_residual_budget` as the new failing-direction fixture.
- **C-E8 (open to the orchestrator).** Scenario (e)'s 8-bit delivery luma mean measures **377 538** against CC6's `DELIVERY_LUMA_MEAN_CODE_8BIT_MILLIONTHS = 400 000` — a **1.06×** margin, 5.6 % of headroom; was §4.1's `185 059 | 2.16x`, taken by probe-1 on the pre-A1 scene and the two-clip (a) document. The other scenarios measure (a) 18 677, (b) 38 177, (c) 195 351, (d) 3 108, (f) 1 760; (e)'s 8-bit RGB mean is 855 810 against 1 750 000 (2.04×) and its PSNR 4 059 against a floor of 3 300, and every lane still reports `within_budgets == true` and `technical_pass == true` at both depths. The **CC6 constant is untouched** — §4.1 note 2 and §4(g)(1) forbid CC7 re-baselining a constant it does not own — and the pre-authorised fallback is that the (e) 8-bit lane becomes **reported, not gated**, if Windows CI overruns it.
- **D-E5 (blocking, resolved).** The §12 step 5 / Q1 measurement blocked steps 5 and 6 until `CC7_MATCH_PROPOSAL_C1` and `CC7_B1_OPERATIONS` were corrected in core; §5.2's (b) script commits C2, so the agent test asserted nothing against the stale constant and the drift had to be caught by measurement rather than by a red test. R2-MAJ-1 closes the remaining half by pinning the corrected constant live in the (b1) leg.
- **F-E6 (accepted).** `cc7_every_color_fixture_builds_a_valid_document` — six real FFmpeg muxes, six probes, six validated documents, ~11 s — is `#[ignore]`d, as every other fixture-build test in `kinewright-eval.rs` is. It is the first test in that binary to construct a real `FfmpegMediaEngine`, whose **process-exit** teardown raises the known SIGSEGV `tests/mcp_server.rs` already lives with (measured on 2 of 3 runs, *after* every test reported ok); §7.8 asks CI for the suite's unit tests only. Run it with `--ignored`; it passes. *Erratum 2026-09-02: root-caused and closed. The SIGSEGV was `FfmpegMediaEngine`'s detached playback worker still inside `libavfilter` (spawning slice threads for the `set_document` proxy render) when process exit ran the FFmpeg and lavapipe finalizers; the engine now joins its workers on drop, and the test runs un-ignored in the default lane.*

**Review outcomes.**

Every BLOCKER and MAJOR is fixed with the reviewer's smallest fix unless the ruling below says otherwise; MINORs are applied unless they contradict the contract or a ruling. Fixers must not add `cc7_` test names beyond those §11.2 already names, and any name added is listed in the fixer's report for step 9b's reconciliation.

- **R1-B1 (blocker).** `cc7_a_fixture_project_path_reaches_the_server` is absent from the tree and R-B2's runner change is executed by no test in any lane (G-E3.2). Ruled: add the contract-named test, factoring the `if let Some(project_path)` branch into `apply_fixture_project_path(&server, &fixture)` and testing it both directions against a real `McpServer` on a synthetic core — `import_lut_asset` refuses `project_not_saved` without, succeeds with; if the real media engine forces `#[ignore]`, the helper is tested with a stub-backed server in the default lane and the ignored variant is kept too.
- **R1-M1.** The leak scan carries neither the run id nor the benchmark id, both of which §8.5 requires. Ruled: the needles take both, and a third failing direction is added.
- **R1-M2.** `chart_luma_mean_delta_millionths` and `gamut_pixel_count` are measured and then discarded — `EvalResult.measurements` is built only from assertions — so c1, c2 and c5 pay extra renders for numbers nobody can read. Ruled: option (a), emit both as `EvalMeasurement { budget: 0, passed: true }` rows so they reach `results.jsonl`; F-E10 stands corrected in the errata.
- **R1-M3.** `ColorEvalEvidence.errors` reaches no artefact unless the quantity is also `None`, so a partially measured quantity can pass with a detail string reporting the requested ROI count. Ruled: non-empty `errors` for a quantity's inputs ⇒ that quantity's assertion **fails** with the recorded reason, with the failing direction added.
- **R1-M4.** `cc7_color_evidence_is_computed_where_the_analysis_is_alive` was renamed **and** narrowed: the tree's test calls `measure_color_evidence` against a stub and never exercises R-B1's plumbing. Ruled: restore the test so it exercises the plumbing, or record honestly what it exercises and rename nothing.
- **R1-M5.** `ground-truth.json` emits `canonical_operations`, not the canonical **documents** §7.7 requires, so the published byte-equality assertion carries no document claim. Ruled: `ground-truth.json` records the documents (ops applied to the initial document) with the equality test.
- **R2 blockers: none.** The review confirmed no response-shape branch lets a refusal pass, no `unwrap_or` in the CC7 block, correct use of the §5.3 template in all nine skip branches, whole-document comparison including keyframes, and the (f) gates reading `observations[]`.
- **R2-MAJ-1.** `CC7_MATCH_PROPOSAL_C1` — the constant D-E5 corrected — is asserted nowhere in the agent suite; the (b1) leg only iterates present controls and `eprintln!`s the integers, so a regression in the real planner is caught by nothing. Ruled: pin `CC7_MATCH_PROPOSAL_C1` live in the (b1) leg (three asserts, absent tint). **Fixed (Y-E1):** five assertions at `mcp_server.rs:2903-2945` — exposure `1 465`, temperature `81`, `tint_percent == 0` with the key absent from both `details` and `parameters`, and `present == ["exposure_milli_stops", "temperature_percent"]` so the iteration cannot pass vacuously — taken against the real `match_parameters`, not the media replica.
- **R2-MAJ-2.** §5.1(5)'s `color_nodes` re-read is present in (a), (c) and (d) only, and the gap is undeclared in the errata. Ruled: add the re-read to (b2), (e) and (f) per §5.1(5); D's errata list is amended. **Fixed (Y-E2):** all six scripts now re-read `color_nodes` — (b2) at `:3059-3089` (empty reference list, all three C2 integers, `assert_ne!` on the raw term), (e) at `:3891-3914`, (f) at `:4449-4487`. (f) deviates from R2's literal "the first keyframed window centre" with the reason stated in the comment: the manifest publishes the node's **stored static** window values (`color_status.rs:3111-3114`) while the tracker writes curves, so the gate is that the manifest still publishes the seeded window at its neutral centre and the contract's half extents.
- **R2-MAJ-3.** D-E9's mitigation sentence is false — `served_surface_is_small_and_keeps_the_internal_registry_discoverable` asserts the count identity and the two `/4` ratios but never compares `serialized_bytes` to a number, and the only two `1_280_060` hits are manifest-against-manifest. Ruled: option (i), one line in that in-crate test asserting `registry_metrics.serialized_bytes == 1_280_060` (and the served `5 660`); D-E9 is corrected. **Fixed (Y-E3):** the tuple assertion at `server.rs:20958-20970`, no other line of `server.rs` moved, and D-E9's closing sentence is rewritten to say the byte count is pinned in-crate **by the R2 fix**, not by the assertion that existed when the erratum was written.
- **R2-MAJ-4.** (b)'s app test builds its own byte-for-byte replica of `inspector_ui.rs:2159`'s slider rather than reading the range the card passes, so E-E3's claim that this *is* §6's "the slider's range equals the descriptor's" overstates it. Ruled: read the card's actual slider range where reachable; otherwise amend E-E3's claim to what the test proves. **Fixed (Y-E4):** the preferred option was reachable — `inspector_ui.rs:8223-8272` seeds a second (b2) node with C2's raw `+248`, draws the real `primary_correction_section` headlessly, and reads the card's readout back through `crate::theme::painted_text`, asserting it reads `primary_parameter_readout("temperature_percent", 100)`, never the raw value, writes nothing, and — as the negative control — reads out the descriptor neutral over an in-range node. E-E3's closing sentence is rewritten to distinguish the descriptor's bound from the card's.
- **R2 minors (Y-E5).** Thirteen applied — descriptor bounds read through the new `cc7_primary_bounds` helper rather than proxied by the clamped value or written as literals; the response's real `f64` `unrounded_delta` read and asserted to round onto `requested`; the blue over-excursion gated by channel; the vacuous (c) node-order loop and the vacuous success-body `assert_ne!` replaced by non-vacuous forms; the inert assertion inside an `abort()`ed task replaced by two `AtomicUsize` counters and `assert_approved_and_stop`; `SKIPPED:` and `applied == false` in all nine skip branches; the (f) inspection frames indexed out of `CC7_TRACK_SAMPLE_FRAMES`; `region_pixel_count` and `out_of_gamut_pixel_count` given separate constants; failing directions added to the (b) and (d) app tests; and `matte_parameter_range`'s self-satisfying `value..=value` path made a failure. Seven declined with reasons, of which one is load-bearing: **m4 is declined because §4(b)(3) and R-M9 say the per-node `+116` is confirmed, never pinned** — the assertion stays `> 0` and the number lives in D-E11 and C-E14 and deliberately nowhere in code. One item is owed: a `CC7_C2_MAX_OVER_EXCURSION_MILLIONTHS` constant if the contract wants `41 538` pinned, since §2.1 forbids restating it at the call site.
- **R2 fixes, names and lanes (Y-E6, Y-E7).** No test name was added, renamed or deleted by the R2 fixes; the eight agent and seven app names were taken from the compiled binaries (`-- --list`), and exactly one — `cc7_d2_a_person_can_author_the_window_only_matte_by_hand` (E-E8) — is not written in this contract. Verified on the default lavapipe lane with `KINEWRIGHT_GPU_TESTS_MAY_SKIP` never set: `--test mcp_server` **unfiltered** 22 passed / 0 failed / 0 ignored, `--lib -- served_surface` 1 passed, `-p kinewright-app -- cc7` 7 passed, `cargo clippy --all-targets -- -D warnings` exit 0 (after three `manual_contains` fixes the new code introduced) and `cargo fmt --check` exit 0. No `cc7_` skip branch took its skip path on this lane.
- **R3 blockers: none.** Every gated number `cc7_sources` authors was re-derived from §2.4's prose in an independent implementation and reproduces the contract exactly, and the mux recipe is byte-for-byte §3.2.
- **R3-M1.** `cc7_camera_scene_rgb(LogLike, ..)` returns the **base** scene while `cc7_camera_source(LogLike)` returns the **log carrier**, and `Cc7SourceKind::Camera(LogLike)` duplicates `Cc7SourceKind::Log` with the same label, frames and content. Ruled: route `LogLike` in ONE place (`cc7_camera_scene_rgb` returns the log scene), and remove the duplicate by normalising to `Log` in a constructor, asserting the two are never both constructible.
- **R3-M2 / M3.** The population fixture's failing-direction block is constant-folding truth that no perturbation of the generator can fire, and the counts are taken over restated literal ranges rather than over the authored raster, contrary to the fixture's own doc comment and §3.5(6). Ruled: count populations over the AUTHORED raster by colour classification — surround, ramp, chart, primaries and row membership decided from the pixel and its position — with real perturbation assertions (`(0,56)` not surround, `(0,72)` surround; a narrowed chart band fails).
- **R3-M4.** `cc7_sources::CC7_CUBE_TITLE` is a second, unreconciled copy of `cc7_scenarios::CC7_LOG_CUBE_TITLE`; a same-length edit to either leaves every assertion green while the LUT record's title and the file it names disagree. Ruled: `pub use kinewright_core::cc7_scenarios::CC7_LOG_CUBE_TITLE` — one constant.

- **R3 fixes (Z-E1–Z-E5).** All four majors were verified on disk by perturbation and the eight minors applied: `LogLike` is routed in `cc7_camera_scene_rgb` alone and `Cc7SourceKind::camera` / `normalized` fold `Camera(LogLike)` into `Log`, asserted never both constructible (deleting the early return fails `cc7_log_source_is_not_the_base_scene`); the population fixture classifies every authored pixel through the patch tables' own `Cc7Patch::rect` and asserts the authored code before counting it, with the edge probes run **first** and in **both** directions (`(0, 55)` surround / `(0, 56)` primary / `(0, 71)` primary / `(0, 72)` surround — a band shifted either way fires by name, a chart band narrowed to `x < 92` fails at `chart11`); `CC7_CUBE_TITLE` is gone in favour of `pub use kinewright_core::cc7_scenarios::CC7_LOG_CUBE_TITLE`; the temp `.yuv` is removed by a `Drop` guard that survives a `run_ffmpeg` panic; static sources author one frame and repeat it (the eight source fixtures run in 1.68 s, from 6.37 s). **§11.2.12's stated failing direction is corrected (Z-E2):** a one-pixel shift of the primaries band does **not** change the surround count — it trades 40 pixels in for 40 out, and every population is invariant — so the failing direction is an **authored code** moving at a named edge, which is what the fixture asserts.
- **R4-B1 (blocker) → W.** `CC7_EVAL_TESTS` and the manifest named the pre-R1 spelling `cc7_color_evidence_is_measured_where_the_analysis_is_alive` and omitted `cc7_a_fixture_project_path_reaches_the_server`, so both inventory tests failed once X's fixes landed. Ruled: step 9b's reconciliation, done last by W after X's report.
- **R4-M1 → Y-E8.** The whole (f) containment gate family in media is a pure function of pinned constants — `cc7_containment_samples()` reads `cc7_track_keyframe_centres`, which is `cc7_stabilized_centres(CC7_TRACK_OBSERVED_CENTRES_BASIS_POINTS, 800)` — and nothing compared the observed table against the real tracker; a 150 bp tracker regression passed both the agent's 200 bp analytic tolerance and the media gate. Ruled: the (f) agent script pins every live observation and confidence against the observed tables and every committed keyframe value against `cc7_track_keyframe_centres`, so the pin is a pin and the media gate keeps its analytic form.
- **R4-M2 → W.** `cc7_every_budget_carries_the_declared_margin` cleared the 2× rule for the 8-bit luma mean on probe-1's pre-A1 `185 059`, while C-E8 measured **377 538** on (e); `CC7_MEASURED_DELIVERY_*` was referenced nowhere outside `cc7_scenarios.rs`. Ruled, without moving a CC6 constant: `CC7_MEASURED_DELIVERY_EIGHT` / `_TEN` are re-pinned to the amended-scene worst per term, the delivery rows that no longer clear 2× carry a `Cc7BudgetKind` that records the margin without asserting it, and `assert_cc7_delivery_lane` asserts each lane's measured term equals the manifest's. **§4.1's table row and note 2 are superseded:** the worst delivery margin is **1.06×** (8-bit luma mean, scenario (e)), not 2.16×, and it is recorded, not cleared.
- **R4 minors.** m1, m2, m3, m4, m5, m7 and m9 fixed as stated (a restated `CC7_CHART_BAND_RECT` literal, a hard-coded lattice size 33, `CC7_MEASURED_FEATHER_MODEL_ERROR_PIXELS = 0` never asserted live, the manifest's `measured` / `margin` asserted numeric rather than equal, the `deep_shadow` ROI sized by `CC7_PRODUCT_PATCH_PIXEL_COUNT`, a literal `1_465` in `cc7_core.rs`, and the transcription cross-check's `1e-6` silently widened to a relative tolerance). **m6:** `CC7_C2_SKIN_IN_BAND_REPORTED_BASIS_POINTS` is re-pinned from **9 411** to **10 000** (C-E9's amended-scene measurement; §2.6, §4(b)(3) and §11.2.22 are superseded). **m8:** the fixture doc comment that still said (b1) shares the (a) budget is corrected; §2.6, §4(b)(1) and §4.1 are superseded by G-E1 above. **m11:** the two REPORTED containment constants are re-pinned to the measured **1 478** / **511** hundredths (C-E6), keeping the ±2 window. **m10 declined:** `Cc7MatchProposal.tint_percent` stays an `i64` whose `0` means *not proposed* under R-M19's absent-key rule; changing it to an `Option` touches three crates mid-fix and the struct doc carries the meaning.
- **R1 fixes (X-E1–X-E9).** R1-B1 was on disk — `apply_fixture_project_path(&server, &fixture)` factored out of `run_eval_with_artifacts`, and `cc7_a_fixture_project_path_reaches_the_server` drives both directions against a real `McpServer` on a synthetic core, not `#[ignore]`d — but the test omitted `client.cancel()` before `server.shutdown()` and left both transports parked for **600.10 s**; two lines take it to 0.10 s. R1-M1: `cc7_leak_needles(run_id, benchmark_id)` carries both identifiers with a third failing direction (a `run_id` breadcrumb on a form entry). R1-M2: `ungated_color_measurements` emits `chart_luma_mean_delta_millionths` and `gamut_pixel_count` as `EvalMeasurement { budget: 0, passed: true }` rows, now asserted; **F-E10 is corrected** — the two quantities reach `results.jsonl` as those rows, not as `ColorEvalEvidence`, which is never serialized. R1-M3: `ColorEvalEvidence.errors` is a `Vec<ColorEvidenceError>` attributed to a `ColorEvidenceQuantity`, and a non-empty set for a quantity's inputs fails that quantity's assertion with the recorded reason, both directions asserted. R1-M4: the test is `cc7_color_evidence_is_computed_where_the_analysis_is_alive` and drives `measure_color_block`, the function the runner calls. R1-M5: `ground-truth.json` now carries `canonical_document` per scenario — the batch applied through `apply_batch` to a synthetic initial document and projected to `clips[].effects` — beside the batch, with (c) and (e) applied through `cc7_lut_backed_canonical_operations` on a placeholder asset record (all-zero digest, `CC7_LOG_CUBE_BYTES_REPORTED`) that never reaches the published bytes; the file was regenerated by the byte-equality test. Minors: eight applied (an unknown `ColorQcCheck` token is a failure, two proofs cannot share a `blind_id` with different bytes, `schema_version` is checked on both the form and the key, the extras are asserted positively per task id), one recorded (**B-E6 is corrected**: `blind_id` and `schema_version: 2` are filled for **every** packaged task, unconditionally, because Q3 requires `blind/` for every packaged run — only the `not_applicable` set is by benchmark id), and one declined (m9 — `EvalMeasurement`'s field set is normative under §7.6 / R-M3). No `cc7_` name was added: `eval.rs` declares 15, `kinewright-eval.rs` 12 plus `published_v6_manifest_tracks_the_color_workflow_suite`.
- **R4-M1 fixed (Y-E8).** In the (f) agent script, every surviving sample's live `[center_x, center_y]` and `confidence_basis_points` are pinned exactly against `CC7_TRACK_OBSERVED_CENTRES_BASIS_POINTS[i]` / `CC7_TRACK_OBSERVED_CONFIDENCE_BASIS_POINTS[i]`, the occluded eleventh row through `low_confidence_samples[0]`, and after the commit every keyframe of every `CC7_F_KEYFRAMED_PARAMETERS` curve is asserted at `CC7_TRACK_SURVIVING_SAMPLE_FRAMES[i]` with value `cc7_track_keyframe_centres(axis)[i]`. The pins agreed with the live tracker on the first run; a +150 bp shift (R4-M1's own scenario) and a +1 keyframe perturbation both fail by name.
<!-- Fixer W (R4 + inventory) outcomes: folded at commit -->

The inventory G measured with step 9b's own both-direction test, rather than transcribing it from the implementer lists, is: `CC7_MEDIA_TESTS` **41** (`cc7_fixtures.rs` 33 + `cc7_sources.rs` 8), `CC7_CORE_TESTS` **12**, `CC7_AGENT_TESTS` **8**, `CC7_APP_TESTS` **7** (`inspector_ui.rs` 6 + `look_browser_ui.rs` 1) and `CC7_EVAL_TESTS` **27** (`eval.rs` 14 + `bin/kinewright-eval.rs` 13) — **95** deduplicated, with `CC7_INVENTORY_TESTS` 2 (both also `CC7_MEDIA_TESTS` members, CC6's overlap), `CC7_EXTERNAL_OWNERS` 9 and `CC7_TEST_SOURCES` 8. The manifest's `required_fixtures` holds **95** entries, one per declared test and asserted exactly equal to that inventory, and its threshold-key count is **84**. Fixer Y added, renamed and deleted no test name (Y-E6), so the R2 fixes leave every count above unchanged. These counts still move with any fixture the remaining fixers add — R1-B1's `cc7_a_fixture_project_path_reaches_the_server` at minimum, plus R1-M1's and R1-M3's failing directions — and are recorded here **(as of G; reconciled at commit)**.

---

## 1. In scope and out of scope

CC7 delivers:

- **`kinewright_core::cc7_scenarios`** (§2) — a public, dependency-free module that is the single authority for the six scenarios: raster geometry in basis points, analytic patch values, the camera transforms, the canonical expected document per scenario, and every named budget constant. Nothing else in the workspace may re-declare one of these numbers.
- **`kinewright_media::cc7_sources`** (§3) — a public (not `cfg(test)`) generator module, in the shape of `test_support`, that authors every CC7 raster in Rust (idiom A) and muxes it FFV1 `-level 3 -g 1`, `yuv444p`, BT.709 limited, `.mkv`. One generator serves the media fixtures, the agent end-to-end tests, and the eval suite.
- **Technical gates as ordinary `cargo test`** (§4) — seven gate families (a)–(g) over the six scenarios, in the default lane, on both CI operating systems, with no model and no network. Each gate names its measuring function, its closed-form sampling rule, its passing bound, and its failing-direction fixture.
- **Six scripted agent end-to-end tests** (§5) — one `cc7_` test per scenario in `crates/kinewright-agent/tests/mcp_server.rs`, driving the real MCP endpoint with scripted tool calls (no LLM), committing through `prepare_edit_plan`/`commit_edit_plan`, and asserting the committed document **equals** `cc7_scenarios`' canonical document.
- **Person-path tests** (§6) — one per scenario (a)–(e) plus a separate window-only (d2) test, in `crates/kinewright-app`, proving the inspector operation builders can express the same canonical operations, that core accepts the batch in order, and that the resulting document equals the canonical document. Scenario (f) is person-N/A by construction and is recorded as a deferral (§13), not as a narrowing.
- **A sixth eval suite `color-workflow-v6`, and the harness plumbing it needs** (§7) — `kinewright-color-workflow-v6`, tasks `c1..c6`, `benchmarks/auto-edit/v6/`, registered in `eval_suite`, `is_packaged_benchmark` and `print_usage`; **`ColorEvalEvidence` and `original_document` on `EvalOutcome`, measured inside `run_eval_with_artifacts`** where the `Analysis` is still alive; **`PreparedFixture.project_path`** and the runner's `set_project_path` call, without which `import_lut_asset` can never succeed in an eval; eight new closed-set `EvalAssertion` variants; `delivery_bit_depth` on `EvalDeliverableSpec`; structured `EvalMeasurement` evidence on `EvalResult`; and `published_v6_manifest_tracks_the_color_workflow_suite`. The last two are **shared-runner changes** and are stated as such.
- **A blind review package** (§8) — `human-review.json` schema_version 2 with a derived `blind_id` and per-task `questions`; a `blind/` directory carrying hashed artefacts **and `blind/review-form.json`, the only file the reviewer opens, keyed on `blind_id` alone**; `blind-key.json` in the run root; `--score-review` resolving the form through the key before binding; a leak test that scans the listing **and the form's bytes**; an explicit statement of what is **not** blinded; and an M40-shaped human gate.
- **`cc7_fixtures.rs` + `cc7_manifest.json`** (§11) — the full CC6-style inventory: cross-crate `CC7_TEST_SOURCES` `include_str!`, both-direction declared-name assertions, key-count assertion, and the `uses_outside_prose` guard against `fixture_gpu_or_skip` and `KINEWRIGHT_GPU_TESTS_MAY_SKIP`.

CC7 does **not** deliver: any new MCP tool or capability (the served surface stays at **7** and `INSPECTOR_TOOL_NAMES` stays at **75**); kelvin or mired white balance, chromatic adaptation, or an auto-WB control; log, LogC, Log3G10, or any camera-native source profile (`classify_source` keeps refusing them, `color.rs:730-739`); a matte edge-quality metric; a noise, grain, or sensor-character measurement; a per-sample track-lost marker or occlusion handling; a live `KinewrightApp` UI harness or a scripted-UI driver; a still-export or split/wipe compare in the app; a new `HumanRatingDimension`; a recorded cross-platform decoded delta; ΔE2000; HDR, BT.2020, PQ, HLG, ACES, OCIO, or RAW; a new delivery lane, codec, or container; and any change to a CC6 budget constant.

---

## 2. The scenario authority — `kinewright_core::cc7_scenarios`

### 2.1 Module boundary

`crates/kinewright-core/src/cc7_scenarios.rs`, `pub mod cc7_scenarios;` in `lib.rs`, re-exported from the crate root. It is **data and arithmetic only**: no `Document` mutation, no rendering, no filesystem, no clock, no RNG. It depends on `crate::scopes::NormalizedRoi`, `crate::operation::Operation`, `crate::effect`'s parameter-name constants, and nothing else.

It exists because six scenarios × three execution paths (media fixture, scripted agent, person builders) × two crates that also read them (agent eval, app) is seven places a patch rectangle or an expected code could drift. **A number that appears in this module must not be restated as a literal anywhere else**; §11.0.3's manifest rule and §11.2's inventory make that checkable.

### 2.2 Public items

```rust
pub enum Cc7Scenario { MixedCamera, WhiteBalance, LogLike, ProductAndSkin, CreativeLook, TrackedSecondary }
pub const CC7_SCENARIOS: [Cc7Scenario; 6];

pub struct Cc7ScenarioSpec {
    pub scenario: Cc7Scenario,
    pub id: &'static str,                       // "a".."f", the eval task suffix
    pub title: &'static str,
    pub width: u32, pub height: u32, pub fps: u32, pub frames: u32,
    pub clips: &'static [Cc7Clip],              // camera per clip, in timeline order
    pub canonical_operations: &'static [Cc7Operation],
    pub human_question: Option<&'static str>,
    pub person_path: Cc7PersonPath,             // Expressible | NotApplicable { reason }
}

pub struct Cc7Clip { pub clip_id: u64, pub camera: Cc7Camera, pub source: Cc7Source }
pub enum Cc7Camera { A, B, C1, C2, LogLike }
pub enum Cc7Source { BaseScene, TrackedSquare }

pub struct Cc7Patch {
    pub name: &'static str,
    pub rect: Cc7PixelRect,                     // half-open, in the 320 x 180 base grid
    pub roi: NormalizedRoi,                     // the bp rect that resolves to `rect`
    pub grade709: Option<[i64; 3]>,             // millionths, None for the chart/primary patches
    pub display_code_cam_a: [u8; 3],
    pub linear_millionths_cam_a: [i64; 3],
}
pub struct Cc7PixelRect { pub x: u32, pub y: u32, pub width: u32, pub height: u32 }

pub const CC7_CHART_PATCHES:   [Cc7Patch; 12];   // ACHROMATIC (A1)
pub const CC7_PRIMARY_PATCHES: [Cc7Patch;  5];   // the primaries band (A1)
pub const CC7_ROW_PATCHES:     [Cc7Patch;  7];

pub struct Cc7CameraTransform {
    pub gain_millionths: [i64; 3],
    pub exposure_milli_stops: i64,
    pub saturation_millionths: i64,             // 1_000_000 = unchanged
}
pub const fn cc7_camera_transform(camera: Cc7Camera) -> Cc7CameraTransform;

pub struct Cc7Operation { pub effect_name: &'static str, pub parameters: &'static [(&'static str, i64)] }

pub const fn cc7_spec(scenario: Cc7Scenario) -> &'static Cc7ScenarioSpec;
pub fn cc7_canonical_operations(scenario: Cc7Scenario) -> Vec<Operation>;   // the exact core batch
pub const fn cc7_tracking_sample_frames() -> [i64; 11];                    // A12, §2.3.6
pub const fn cc7_analytic_square_top_left(frame: i64) -> (i64, i64);       // amplitude (100, 40)
pub const fn cc7_analytic_square_centre_basis_points(frame: i64) -> (i64, i64);
pub const fn cc7_log_encode_code(linear_millionths: i64) -> u8;
pub const fn cc7_log_inverse_display(v_millionths: i64) -> i64;            // display709, millionths
```

`cc7_canonical_operations` returns the operations in the order a commit must apply them; it is the **single** definition of "the canonical document" and is consumed by §4, §5, §6, and §7 alike.

### 2.3 Raster geometry

**2.3.1 The shared scene.** Every scenario renders `320 × 180` at `25` fps. Scenarios (a)–(e) and (g) are **60** frames (`CC7_SOURCE_FRAMES`); (f) is **100** (`CC7_TRACK_FRAMES`). The size and rate are CC6's (`cc6_fixtures.rs:297-302`), so the encoder GOP is `2 · fps = 50` and CC6's five-frame delivery sample set `0, 14, 29, 44, 59` still spans two GOPs. A two-clip (a)/(b) document is 120 frames and samples `0, 29, 59, 89, 119`.

**2.3.2 The basis-point conversion, normative.** `NormalizedRoi::to_pixels` floors the start and **ceils** the exclusive end (`crates/kinewright-core/src/scopes.rs:245-300`). For a target half-open pixel rect `[p0, p1)` on an extent `E`, CC7 pins

```text
start_bp = ceil(p0 · 10_000 / E)      end_bp = floor(p1 · 10_000 / E)
width_bp = end_bp − start_bp
```

and asserts `to_pixels(320, 180)` recovers `[p0, p1)` exactly for every rect in this section. **The `ceil` on the start is load-bearing** (A19): probe-2 measured that the naive `(2250, 4222, 375, 888)` for the `deep_shadow` patch — `76 · 10000/180 = 4222.2̄` truncated — resolves to pixel `y 75, h 17`, **204** pixels rather than 192, because the floored start lands one row early. Every CC7 ROI uses the `ceil`ed start (`4223`), and §11.2.1 asserts the resolved pixel rect for each, not merely the arithmetic. `10_000/320 = 31.25` is exact on 4-pixel boundaries; `10_000/180 = 55.5̄` is exact only on 9-pixel boundaries, and the row boundaries (20, 36, 52, 56, 72, 76, 92) are not multiples of 9 — the floor/ceil rule is what makes them exact anyway, and §11.2.1 asserts the round trip rather than a divisibility that does not hold.

**2.3.3 The base scene (A1)**, by row, in the `320 × 180` grid. Everything unnamed is the achromatic surround, display code **115** (`CC7_SURROUND_CODE`, `round(255 · 0.450_148) = 115`, the display encoding of CC5's `CHART_SURROUND` `[0.45, 0.45, 0.45]` grade709, `cc5_fixtures.rs:4639`).

| Region | Pixel rect (half-open) | `NormalizedRoi` (x, y, w, h) bp | Pixels | Content |
| --- | --- | --- | ---: | --- |
| neutral ramp band | `x 0..320, y 0..20` | `(0, 0, 10000, 1111)` | 6 400 | `grey(x · 255 / 319)`, integer division |
| **achromatic chart band** | `x 0..96, y 36..52` | `(0, 2000, 3000, 888)` | 1 536 | `CC7_CHART_PATCHES`, 8 px each, x origin **0** |
| **primaries band** | `x 0..40, y 56..72` | `(0, 3112, 1250, 888)` | 640 | `CC7_PRIMARY_PATCHES`, 8 px each, x origin **0** |
| patch row | `x 0..84, y 76..92` | `(0, 4223, 2625, 888)` | 1 344 | `CC7_ROW_PATCHES`, 12 px each, x origin **0** |
| surround | remainder | — | 47 680 | code 115 |

`6 400 + 1 536 + 640 + 1 344 + 47 680 = 57 600 = 320 · 180`, asserted by `cc7_base_scene_populations_are_the_contract_table`.

**Chart patch `k`** (`k = 0..=11`) occupies `x 8k..8k+8, y 36..52`, `roi = (250k, 2000, 250, 888)`, **128** pixels. The twelve display codes are **`[0, 11, 24, 48, 72, 104, 128, 152, 180, 208, 242, 255]`** — CC1's six reference steps plus six intermediates — and **every one is achromatic**, which is what makes (a)'s spread statistic meaningful over the whole band and (d)'s exact containment reachable (A1).

**Primary patch `k`** (`k = 0..=4`) occupies `x 8k..8k+8, y 56..72`, `roi = (250k, 3112, 250, 888)`, **128** pixels, in the order `[0,255,0] [0,0,255] [0,255,255] [255,0,255] [255,255,0]`. **The pure red `[255,0,0]` is deliberately absent** (A1): probe P5 measured the derived `product_red` qualifier at hue `35 865 ± 1 500` centidegrees with `1 000` cd softness, and the red primary's grade709 hue of `0` cd sits 135 cd from that centre, so it is captured and (d)'s "exactly 192" could not pass. Magenta (`30 000` cd) and yellow (`6 000` cd) are more than 2 500 cd away and stay. The blue primary stays because it is the population that clips in (b2) (probe P2).

**Row patch `j`** (`j = 0..=6`) occupies `x 12j..12j+12, y 76..92`, `roi = (375j, 4223, 375, 888)`, **192** pixels, in the order `skin_light, skin_medium, skin_tan, skin_deep, product_red, product_cyan, deep_shadow`. `deep_shadow` is the **seventh column of the same row**, immediately to the right of `product_cyan`.

Derived region ROIs used by the gates: the **four skin patches** `(0, 4223, 1500, 888)` = `x 0..48`; **`product_red`** `(1500, 4223, 375, 888)` = `x 48..60`, `CC7_PRODUCT_PATCH_PIXEL_COUNT = 192`; **`deep_shadow`** `(2250, 4223, 375, 888)` = `x 72..84`, 192 pixels.

**2.3.4 Scenario (a)/(b) documents.** Two clips on one video track, clip 1 then clip 2, each 60 frames, `timeline_start` 0 and 60. Clip 1 is always the reference camera A; clip 2 is B (scenario a), C1, or C2 (scenario b). The two clips reference **two distinct encodes**, never one asset split in half: `two_shot_color_document` (`tests/mcp_server.rs:489-500`) is colorimetrically vacuous and CC7 does not reuse it.

**2.3.5 Scenario (c)/(d)/(e) documents.** One clip, 60 frames, camera A (d, e) or LogLike (c).

**2.3.6 Scenario (f) raster and geometry — measured (A12, A14, A17, A18).**

100 frames. The base is the surround plus the **four static skin patches at `y 4..20`, `x 0..48`** (12 px each) and nothing else; the ramp, chart, and primaries bands are omitted so the tracked template carries one moving feature and one static one. A `product_red` square, `CC7_TRACK_SQUARE_SIZE = 24` px on a side at the `product_red` display code `[204, 26, 31]`, is drawn **last** (opaque, on top) with top-left

```text
x(f) = round(148 + 100 · sin(2π f / 100))        y(f) = round(78 + 40 · sin(2π f / 100))
CC7_TRACK_CENTRE_X_PIXELS = 148   CC7_TRACK_AMPLITUDE_X_PIXELS = 100
CC7_TRACK_CENTRE_Y_PIXELS =  78   CC7_TRACK_AMPLITUDE_Y_PIXELS =  40
```

integer-rounded, no 4:2:0 snapping (the source is `yuv444p`). The generator asserts `0 ≤ x`, `x + 24 ≤ 320`, `y ≥ 24` (so the square never touches the `y 4..20` patch rows) and `y + 24 ≤ 180` at every frame.

**The brief's `(100, 40)` amplitude is kept** (A12). Probe-2 built the drafter's slower `(60, 30)` variant as well and measured it **worse on every gated term**: worst raw observation error 104 bp against 49, worst smoothed error 90 bp against 87, and a larger required containment half-extent in y. `MATTE_TRACK_MAX_STEP_BASIS_POINTS = 800` is reached exactly once on this path — the `4 → 9` segment, raw Δx 898 bp clamped to 800 — and the clamp self-corrects at the next sample at a net cost of **≤ 98 bp** to the smoothed curve. Draft v2's concern was real and two orders of magnitude smaller than a containment failure.

**Occlusion:** on frames `CC7_TRACK_OCCLUSION_FIRST_FRAME = 43 ..= CC7_TRACK_OCCLUSION_LAST_FRAME = 47` the square is **not drawn**; those pixels are surround.

**The tracked range and its sample set (A12).** The (f) call is

```text
start_local_frame = 0   end_local_frame = 48   step_frames = CC7_TRACK_STEP_FRAMES = 5
search_radius_percent = CC7_TRACK_SEARCH_RADIUS_PERCENT = 10   max_width = CC7_TRACK_MAX_WIDTH = 256
```

`tracking_sample_frames` (`server.rs:11810-11845`) does **not** step by `step_frames`: it treats `step` as a *maximum* spacing, distributes `ceil(span / step)` intervals **evenly** over `start ..= end − 1` as `f_i = start + floor(span · i / interval_count)`, and appends `last`. For `0..48` at step 5 that is `interval_count = ceil(47/5) = 10` and

```text
CC7_TRACK_SAMPLE_FRAMES = [0, 4, 9, 14, 18, 23, 28, 32, 37, 42, 47]     (11 samples)
```

transcribed independently and asserted equal to the tool's `observations[].local_frame`. **The range ends at the occlusion on purpose** (A12, §13): `track_matte_window` has **no re-acquisition**, so a range that continues past frame 47 returns frozen positions at confidence `10 000` — measured up to **5 176 bp** (165 px) from the subject by frame 74 on the `0..100` grid. That is the tool's published `MATTE_TRACKING_BOUNDARY` ("no occlusion handling"), not a defect, and **no CC7 gate may span an occlusion**.

**`search_radius_percent = 10` is pinned with its reason** (A18): the per-sample motion is ≤ 25 thumbnail pixels, inside the 10 % radius of 25.6 px on a 256-wide thumbnail. Probe-2 measured every observation, confidence and keyframe **bit-identical** at 10 % and 25 %, so CI runs one radius, not two.

**Analytic centres.** `centre = (x + 12, y + 12)` px, in basis points of the composite, `round(cx · 10000/320)` and `round(cy · 10000/180)` with §10.1's half-away-from-zero rule — which is load-bearing at frames 18, 28 and 32, where the exact value is `…12.5` bp:

| f | top-left | centre px | centre bp | | f | top-left | centre px | centre bp |
| ---: | --- | --- | --- | --- | ---: | --- | --- | --- |
| 0 | (148, 78) | (160, 90) | (5000, 5000) | | 28 | (246, 117) | (258, 129) | (8063, 7167) |
| 4 | (173, 88) | (185, 100) | (5781, 5556) | | 32 | (238, 114) | (250, 126) | (7813, 7000) |
| 9 | (202, 99) | (214, 111) | (6688, 6167) | | 37 | (221, 107) | (233, 119) | (7281, 6611) |
| 14 | (225, 109) | (237, 121) | (7406, 6722) | | 42 | (196, 97) | (208, 109) | (6500, 6056) |
| 18 | (238, 114) | (250, 126) | (7813, 7000) | | **47** | (167, 85) | (179, 97) | (5594, 5389) *occluded* |
| 23 | (247, 118) | (259, 130) | (8094, 7222) | | | | | |

`CC7_TRACK_ANALYTIC_CENTRES_BASIS_POINTS` is this table; `CC7_TRACK_EXPECTED_LOW_CONFIDENCE_FRAMES = [47]`.

**The seeded window and the containment window (A17).** The node's stored window is centred on frame 0's square — `center_x = 5_000`, `center_y = 5_000`, `half_width = 375`, `half_height = 667` (`round(12/320 · 10000)` and `round(12/180 · 10000)`), `shape_token` at its neutral `1` (rect). Frame 0's square is exactly `x 148..172, y 78..102`, whose continuous centre is `(160.0, 90.0)` = `(5 000, 5 000)` bp exactly. `box_percent` resolves to `[8, 13]`, inside `track_matte_window`'s `1..=75` rule.

That 1.0× window does **not** contain the moving square once tracked: probe-2 measured the worst required half-extent at **14.77 px (462 bp) in x and 12.88 px (716 bp) in y**, so the seeded 12 px window is 2.77 px short in x. The **containment gate therefore uses a 1.5× window**:

```text
CC7_TRACK_WINDOW_HALF_WIDTH_BASIS_POINTS  =   563     (18 px, round(18·10000/320) = 563)
CC7_TRACK_WINDOW_HALF_HEIGHT_BASIS_POINTS = 1_000     (18 px, 18·10000/180 exactly)
CC7_TRACK_CONTAINMENT_REQUIRED_HALF_WIDTH_PIXELS_REPORTED   = 1_477   (14.77 px, hundredths)
CC7_TRACK_CONTAINMENT_REQUIRED_HALF_HEIGHT_PIXELS_REPORTED  = 1_288   (12.88 px, hundredths)
CC7_TRACK_CONTAINMENT_WORST_MARGIN_X_PIXELS_HUNDREDTHS      =   323   (3.23 px, reported)
CC7_TRACK_CONTAINMENT_WORST_MARGIN_Y_PIXELS_HUNDREDTHS      =   511   (5.11 px, reported; was 512 — §0.3 R4-m11)
```

### 2.4 Analytic patch values

**2.4.1 Camera A (identity).** Row-patch codes are `round(255 · encode_bt709(grade709_decode(g)))` — grade709 → linear → display709 → round. **`decode_display709` does not appear in this path**; it appears in §2.4.2's carrier derivation and in §2.4.3's camera transforms, which start from a *display code*. `grade709_decode` is `color_pipeline.rs:975-985`; `encode_bt709` is `color_pipeline.rs:354-364`. The two encodings differ only in the fourth decimal (`GRADE709_ALPHA 1.099_296_8` against the rounded `1.099`), so **every patch's code equals `round(255 · g)` as well**; the fixture computes the stated path and asserts the agreement, which is a free cross-check and not a substitute for it.

| Patch | grade709 | linear (f64) | display709 | code (cam A) |
| --- | --- | --- | --- | --- |
| `skin_light` | 0.85, 0.68, 0.60 | 0.721 798, 0.465 557, 0.365 963 | 0.850 040, 0.680 086, 0.600 108 | **217, 173, 153** |
| `skin_medium` | 0.72, 0.53, 0.44 | 0.520 332, 0.289 498, 0.205 445 | 0.720 076, 0.530 127, 0.440 151 | **184, 135, 112** |
| `skin_tan` | 0.55, 0.38, 0.30 | 0.310 342, 0.158 076, 0.105 347 | 0.550 121, 0.380 167, 0.300 189 | **140, 97, 77** |
| `skin_deep` | 0.32, 0.20, 0.15 | 0.117 434, 0.055 516, 0.036 983 | 0.320 184, 0.200 216, 0.150 229 | **82, 51, 38** |
| `product_red` | 0.80, 0.10, 0.12 | 0.640 023, 0.022 489, 0.027 814 | 0.800 054, 0.100 243, 0.120 238 | **204, 26, 31** |
| `product_cyan` | 0.10, 0.65, 0.75 | 0.022 489, 0.426 664, 0.563 622 | 0.100 243, 0.650 094, 0.750 067 | **26, 166, 191** |
| `deep_shadow` | 0.05, 0.05, 0.05 | 0.011 111 ×3 | 0.050 000 ×3 | **13, 13, 13** |
| surround | 0.45, 0.45, 0.45 | 0.214 006 ×3 | 0.450 148 ×3 | **115, 115, 115** |

Chart and primary codes are display codes directly. **The decode round trip costs at most one code on a flat patch** (probe-1: source 11 reads back as 10 on the monitoring raster), and every patch statistic in §4 is taken on a **2-pixel inset** of its rect so patch edges are excluded.

**2.4.2 The log-like curve (scenario c).** Content is the base scene's **linear** values run through

```text
v(x) = clamp((log2(x) + 8) / 12, 0, 1)              stored code = round(255 · v)
CC7_LOG_OFFSET_STOPS = 8        CC7_LOG_SPAN_STOPS = 12
```

**The curve is fed the analytic grade709 linear, not the decoded 8-bit code.** Feeding the decoded code instead gives `skin_light 160,146,139`, `skin_deep 105`, `product_red …,61`, `deep_shadow 33` — visibly different, and wrong; the generator takes the analytic path and §11.2.4 asserts the analytic codes. Anchors: `v(1.0) = 2/3 = 0.666 667` → code **170**; `v(0.18) = 0.460 506` → code **117**. The brief's `0.4589` for 18 % grey did not satisfy its own formula and is superseded (the formula is the authority). The curve's floor is `x = 2^−8 = 0.003 906 25`; every `x` below it stores `v = 0`.

The twelve achromatic chart patches through the curve, and back through an **exact** inverse (no lattice):

| chart code | linear | `v` | stored log code | exact inverse → code | error |
| ---: | ---: | ---: | ---: | ---: | ---: |
| 0 | 0.000 000 | 0.000 000 | **0** | 4 | **+4** |
| 11 | 0.009 586 | 0.107 929 | **28** | 11 | 0 |
| 24 | 0.020 981 | 0.202 104 | **52** | 24 | 0 |
| 48 | 0.050 697 | 0.308 169 | **79** | 48 | 0 |
| 72 | 0.095 172 | 0.383 890 | **98** | 72 | 0 |
| 104 | 0.179 084 | 0.459 893 | **117** | 103 | −1 |
| 128 | 0.261 482 | 0.505 398 | **129** | 128 | 0 |
| 152 | 0.361 292 | 0.544 270 | **139** | 153 | +1 |
| 180 | 0.500 507 | 0.583 455 | **149** | 181 | +1 |
| 208 | 0.665 016 | 0.617 622 | **157** | 206 | −2 |
| 242 | 0.899 828 | 0.653 977 | **167** | 243 | +1 |
| 255 | 1.000 000 | 0.666 667 | **170** | 255 | 0 |

**The black patch's +4 is a property of the curve, not of the LUT** (A2): `v = 0` inverts to `2^−8` linear, which monitors as code 4, and no lattice size changes that because the forward curve is not invertible at 0. Row patches through the same curve store `skin_light 160,147,139`, `skin_medium 150,132,121`, `skin_tan 134,113,101`, `skin_deep 104,81,69`, `product_red 156,54,60`, `product_cyan 54,144,152`, `deep_shadow 32,32,32`, surround `123,123,123`.

**2.4.3 Cameras B, C1, C2.** Applied **in linear light** in Rust: decode display709 → per-channel gain → exposure → saturation → encode display709 → round. Saturation is the Rec.709 luma mix in linear, `out = luma + s·(in − luma)` with `luma = 0.2126 R + 0.7152 G + 0.0722 B`.

```text
code_out(c) = round(255 · encode_bt709( sat( expo( gain( decode_display709(c/255) ) ) ) ))
```

| Camera | `gain_millionths` | exposure | saturation |
| --- | --- | --- | --- |
| A | 1 000 000, 1 000 000, 1 000 000 | 0 | 1 000 000 |
| B | 1 060 000, 1 000 000, 940 000 | −500 milli stops (`2^−0.5`) | 850 000 (`×0.85`) |
| C1 | 920 000, 1 000 000, 1 080 000 | −1 500 milli stops | 1 000 000 |
| C2 | 800 000, 1 000 000, 1 250 000 | −2 500 milli stops | 1 000 000 |

Measured source codes on the six reference chart steps and `deep_shadow` (P1; **cam A's row is re-confirmed on the amended scene by probe-3**, which built the identical raster and printed the seven-patch row byte for byte. The amended chart adds six intermediate steps and the primaries band; those are **new rows** of this table for cameras B, C1 and C2, not changes to these, and they are measured at §12 step 5, A20):

| cam | 0 | 11 | 104 | 180 | 242 | 255 | `deep_shadow` |
| --- | --- | --- | --- | --- | --- | --- | --- |
| A | 0,0,0 | 11,11,11 | 104,104,104 | 180,180,180 | 242,242,242 | 255,255,255 | 13,13,13 |
| B | 0,0,0 | 8,8,7 | 88,85,83 | 154,150,146 | 209,204,198 | 220,215,209 | 10,9,9 |
| C1 | 0,0,0 | 4,4,4 | 53,56,59 | 99,103,108 | 136,142,148 | 144,150,156 | 4,5,5 |
| C2 | 0,0,0 | 2,2,2 | 28,34,40 | 60,69,79 | 86,97,110 | 91,103,117 | 2,2,3 |

`CC7_CAMERA_PATCH_CODES` holds the full `(12 chart + 5 primary + 7 row) × 3` table per camera; probe-2 fills the twelve intermediate and five primary rows on the amended scene.

**Proposals `match_parameters` produces**, measured by re-implementing `color_scopes.rs:1866-1962` verbatim (B and C2 on the amended twelve-patch achromatic ROI, P2; C1 on the pre-amendment six-patch band, P1):

| candidate | ROI used | exposure | temperature | tint | clamped |
| --- | --- | ---: | ---: | ---: | --- |
| B | twelve achromatic patches | **+477** | **−45** | **+6** | none |
| C1 | six-patch grey band (P1; re-measured at §12 step 5) | +1 432 | +77 | −3 | none |
| C2 | twelve achromatic patches | **+2 410** | **+100** (raw **+248**) | **−30** | temperature only |

The B and C2 rows are probe-2's measurements on the **amended** scene and are the proposals every (a) and (b2) gate below is stated against; C1 was measured only on the pre-amendment six-patch band and is re-measured at §12 step 5 (A20).

### 2.5 Canonical operations per scenario

Exactly the batch a commit must land. Effect names from `effect.rs:1492-1510`; parameter names from `effect.rs:151-232` and `effect.rs:562-630`.

| Scenario | Target clip | Canonical operations |
| --- | --- | --- |
| (a) | clip 2 (cam B) | one `InsertEffect` `primary_correction` carrying `exposure_milli_stops = 477`, `temperature_percent = −45`, `tint_percent = 6` (P2); **no `saturation_percent`**, **no operation on clip 1** |
| (b1) | clip 2 (cam C1) | one `InsertEffect` `primary_correction` carrying `exposure_milli_stops = 1432`, `temperature_percent = 77`, `tint_percent = −3` (P1; re-measured at §12 step 5) |
| (b2) | clip 2 (cam C2) | one `InsertEffect` `primary_correction` carrying `exposure_milli_stops = 2410`, `temperature_percent = 100` (the clamp), `tint_percent = −30` (P2) |
| (c) | clip 1 (LogLike) | `AddLutAsset` (via `import_lut_asset`) then one `InsertEffect` `technical_lut` at the input stage carrying **only** `lut_asset_id`; `input_encoding_token = 0` is the descriptor neutral and is **not stored** (`color_status.rs:4205-4216`) |
| (d) | clip 1 (cam A) | one `InsertEffect` `primary_correction` carrying `saturation_percent = 40`, `matte_enabled = 1`, and the nine `matte_*` qualifier parameters derived from the `product_red` sample; **no window** |
| (d2) | clip 1 (cam A) | a **separate, window-only** node: one `InsertEffect` `primary_correction` carrying `saturation_percent = 40`, `matte_enabled = 1`, `matte_window_count = 1`, `matte_window0_shape_token = 1`, `_center_x_basis_points = 1687`, `_center_y_basis_points = 4666`, `_half_width_basis_points = 187`, `_half_height_basis_points = 444`, `_feather_basis_points = 1000` — and **no qualifier** (`matte_qualifier_enabled` left at its neutral, so it is not stored) |
| (e) | clip 1 (cam A) | one `InsertEffect` `creative_look` at the look stage carrying **only** `lut_asset_id` (the built-in `warm` asset); `mix_basis_points = 10_000` is the neutral and is not stored (`effect.rs:214-228`) |
| (f) | clip 1 | one `InsertEffect` `primary_correction` with `saturation_percent = 40`, `matte_enabled = 1`, `matte_window_count = 1`, `matte_window0_center_x_basis_points = 5000`, `_center_y_basis_points = 5000`, `_half_width_basis_points = 375`, `_half_height_basis_points = 667` (§2.3.6), then two `SetEffectKeyframes` on `matte_window0_center_x_basis_points` and `matte_window0_center_y_basis_points` (`server.rs:4631-4646`) |

`(d)`'s qualifier is `derive_qualifier_from_sample` over `sample_roi = (1500, 4223, 375, 888)` with the pinned constants `MATTE_SAMPLE_HUE_WIDTH_CENTIDEGREES = 1_500`, `MATTE_SAMPLE_SOFTNESS = 1_000`, `MATTE_SAMPLE_BAND_MARGIN_BASIS_POINTS = 1_000` (`color_status.rs:4390-4400`). Probe-1 measured the sample statistics `hue_median = 35_865` cd, `sat p10 = p90 = 8_728` bp, `luma p10 = p90 = 2_513` bp over 192 visible, 192 chromatic, 0 achromatic pixels, and the derived qualifier **hue `35_865 ± 1_500` softness `1_000`; saturation `7_728..9_728` softness `1_000`; luma `1_513..3_513` softness `1_000`** (P5). The `product_red` sample ROI is untouched by A1, so probe-2 committed the same qualifier directly and confirmed its coverage on the amended scene (P9).

**(d) and (d2) are two nodes and two canonical documents (R-B4)**, not one node with a window added. `Matte::coverage` **multiplies** the legs — `raw = self.window_weight(uv, aspect) * self.qualifier.map_or(1.0, |q| q.weight(rgb_in))` (`crates/kinewright-media/src/color_pipeline.rs:2109-2112`) — so a node carrying both would intersect the 192-pixel qualifier set with the window and yield `covered 192 / full 140 / partial 52`, not the feather band §4(d)(4) measures. (d) is **qualifier-only**; (d2) is **window-only**.

`(d2)`'s window is the `product_red` patch's own rect, at the basis points probe-1 measured against: centre `(1687, 4666)`, half-extents `(187, 444)`, which resolve to `cx = 53.984`, `cy = 83.988`, `hw = 5.984`, `hh = 7.992` pixels. `CC7_FEATHER_BASIS_POINTS = 1_000`.

### 2.6 Budget constants

Every threshold CC7 gates on is a `SCREAMING_SNAKE` constant in this module with its unit in the name. **No CC7 gate uses a literal, and no CC7 constant is a float**; fractional terms are `_MILLIONTHS`, rates are `_BASIS_POINTS`, angles are `_CENTIDEGREES`, counts are plain integers with `_PIXELS` or `_CODE`.

```text
CC7_MATCH_NEUTRAL_SPREAD_MAX_CODE                    =         5   (8-bit monitoring codes)  A8/A15
CC7_MATCH_LUMA_MEAN_MAX_CODE_MILLIONTHS              = 5_000_000   (codes, millionths)           A8
CC7_LOG_FIRST_PERCENTILE_MIN_CODE16                  =     5_140   (16-bit codes = 20 x 257) A9/A21
CC7_LOG_P99_MAX_CODE16                               =    51_400   (16-bit codes = 200 x 257)A9/A21
CC7_LOG_INVERSE_MAX_CODE                             =        12   (8-bit monitoring codes)   A2/A22
CC7_LOG_CUBE_SIZE                                    =        65   (lattice points per axis)  A2/A22
CC7_FEATHER_PARTIAL_TOLERANCE_PIXELS                 =         4   (pixels)                      A7
CC7_LOOK_DEEP_SHADOW_OUT_OF_GAMUT_PIXELS             =       192   (pixels, 12 x 16)             A3
CC7_TRACK_MIN_CONFIDENCE_BASIS_POINTS                =     8_500   (basis points)               A14
CC7_TRACK_TOLERANCE_BASIS_POINTS                     =       200   (basis points, CC5's)        A14
CC7_TRACK_RANGE_END_LOCAL_FRAME                      =        48   (frames, exclusive)          A12
CC7_TRACK_F2_STEP_FRAMES                             =        47   (frames, the (f2) recipe)    A13
```

Reported, never gated:

```text
CC7_UNRECOVERABLE_RESIDUAL_SPREAD_REPORTED_CODE      =        19   (corrected C2, A15; was 17)
CC7_C2_SKIN_IN_BAND_REPORTED_BASIS_POINTS            =    10_000   (corrected C2; was 9_411 — §0.3 C-E9 / R4-m6)
CC7_C2_OVER_RANGE_PIXELS_REPORTED                    =       672   (blue channel only, A16)
CC7_C2_OVER_RANGE_BASIS_POINTS_REPORTED              =       116   (A16; was 22 on the pre-A1 scene)
CC7_WARM_WHOLE_RASTER_OUT_OF_GAMUT_PIXELS_REPORTED   =     1_480   (A19; 1 608 pre-A1)
CC7_WARM_WHOLE_RASTER_OUT_OF_GAMUT_BASIS_POINTS      =       256   (A19; 279 pre-A1)
CC7_LOG_BLACK_PATCH_REPORTED_CODE                    =         4   (every lattice size, A22)
CC7_LOG_PRIMARY_REPORTED_CODE                        =         5   (size 65, not gated, A22)
CC7_LOG_IDENTITY_CUBE_REPORTED_CODE                  =        85   (33^3 identity, A22)
CC7_LOG_CUBE_BYTES_REPORTED                          = 7_414_990   (65^3 canonical .cube, A22)
CC7_TRACK_OCCLUDED_CONFIDENCE_MAX_REPORTED           =     7_411   (basis points, A14)
CC7_TRACK_CLEAN_CONFIDENCE_MIN_REPORTED              =     9_740   (basis points, A14)
```

Exact constants, derived rather than measured:

```text
CC7_SOURCE_WIDTH  = 320      CC7_SOURCE_HEIGHT = 180      CC7_SOURCE_FPS = 25
CC7_SOURCE_FRAMES = 60       CC7_TRACK_FRAMES  = 100      CC7_SURROUND_CODE = 115
CC7_CHART_PATCH_WIDTH = 8    CC7_ROW_PATCH_WIDTH = 12     CC7_CHART_PATCH_PIXELS = 128
CC7_PRIMARY_PATCH_COUNT = 5  CC7_CHART_PATCH_COUNT = 12   CC7_ROW_PATCH_PIXELS = 192
CC7_PRODUCT_PATCH_PIXEL_COUNT = 192                       CC7_TRACK_SQUARE_SIZE = 24
CC7_TRACK_STEP_FRAMES = 5    CC7_TRACK_SEARCH_RADIUS_PERCENT = 10   CC7_TRACK_MAX_WIDTH = 256
CC7_TRACK_OCCLUSION_FIRST_FRAME = 43   CC7_TRACK_OCCLUSION_LAST_FRAME = 47
CC7_FEATHER_BASIS_POINTS = 1_000       CC7_SECONDARY_SATURATION_PERCENT = 40
CC7_LOOK_MIX_BASIS_POINTS = 10_000     CC7_LOG_OFFSET_STOPS = 8      CC7_LOG_SPAN_STOPS = 12
CC7_LOG_FLOOR_LINEAR_MILLIONTHS = 3_906
CC7_LOOK_BLUE_ZERO_CROSSING_DISPLAY709_MILLIONTHS = 74_074        (linear 16_461 millionths)
CC7_SKIN_IN_BAND_EXACT_BASIS_POINTS = 10_000           CC7_MATTE_OUTSIDE_CHANGED_PIXELS_MAX = 0
CC7_DELIVERY_ALLOWED_INFO_CODES = ["delivery_tag_not_representable"]                       A6
CC7_DELIVERY_LEG_BUDGET_SECONDS_LINUX = 90                                                 A10
```

`CC7_LOOK_BLUE_ZERO_CROSSING_DISPLAY709_MILLIONTHS` carries its encoding in its name because the number is a **display709** value, not a linear one (§2.6's own unit rule). It is the built-in `warm` look's blue zero crossing: `Warm[2] = (e2 − 0.5)·1.08 + 0.46` (`builtin_looks.rs:167-176`), so the output is negative for `e2 < 0.5 − 0.46/1.08 = 0.074_074_1`, i.e. linear `< 0.016_460_9`. Green crosses at `0.037_037` and red at exactly `0` (never), which is why the cyan primary `[0,255,255]` sits on the boundary and is not counted (probe P4).

**Distinctness, normative.** Every CC7 budget above is asserted numerically distinct from `MONITOR_CPU_GPU_{MAX, P99, MEAN}` (`cc1_fixtures.rs:62-67`), from `DELIVERY_CODEC_{MAX,P99,MEAN}` (`:68-70`), from every `DELIVERY_*` constant of CC6 §6.3, and from `MATTE_TRACK_MAX_STEP_BASIS_POINTS = 800` and `DEFAULT_MATTE_TRACK_MINIMUM_CONFIDENCE_BASIS_POINTS = 5_000` (`server.rs:11192-11204`). `MATTE_TRACK_DEAD_ZONE_BASIS_POINTS` is deliberately **0** (`server.rs:11192`), so distinctness from it is trivially true for every positive constant and CC7 does not assert it, by `cc7_budgets_are_distinct_from_every_neighbouring_constant`; `CC7_TRACK_MIN_CONFIDENCE_BASIS_POINTS != 5_000` is asserted by name, because probe-2 measured that the default drops nothing. **Every CC7 budget carries ≥ 2× margin over the Linux measurement**; there is one constant per term and never a per-OS constant (R5).

### 2.7 What `cc7_scenarios` is not

It is not a renderer, not a fixture, and not a test helper: it holds no `Analysis`, spawns no `Core`, and reads no file. It does not re-implement `measure_color_qc`, `match_parameters` (**`crates/kinewright-agent/src/color_scopes.rs:1860-1965` — the agent crate, not core**), `bt709_limited_ycbcr`, or the compositor; §11.0.1 forbids a fixture from obtaining an expected value by calling any of them, and this module's job is to be the place the analytic value is written down instead.

**It does carry three transfer transcriptions, and it must (R-M2).** `crates/kinewright-core/Cargo.toml` has **no path dependency on `kinewright-media`**, so `encode_bt709`, `decode_display709` and `grade709_decode` (`crates/kinewright-media/src/color_pipeline.rs`) and `SPEC_F64_TOLERANCE` (`cc1_fixtures.rs`, `pub(crate)`) are unreachable from `cc7_scenarios` and from `cc7_core.rs` alike. `cc7_scenarios` therefore carries its **own `f64` transcription** of those three functions and its own `SPEC_F64_TOLERANCE`, each with a comment naming the owning module and contract section — exactly CC6's precedent, which wrote `const SPEC_F64_TOLERANCE: f64 = 1e-6;` into `crates/kinewright-core/tests/cc6_core.rs:35-37` with the same explanation. The transcription is held honest from the media side by **`cc7_core_transcriptions_agree_with_the_pipeline`** (§11.2.12b), which asserts agreement with `color_pipeline`'s real functions at every patch value within `1e-6`, in both directions. A transcription nobody cross-checks is a second definition; a transcription with a cross-check is a boundary.

---

## 3. The source generators — `kinewright_media::cc7_sources`

`crates/kinewright-media/src/cc7_sources.rs`, `pub mod cc7_sources;`. **Public, not `cfg(test)`**, in the shape of `test_support` (`crates/kinewright-media/src/test_support.rs:19-297`, itself `pub mod` at `lib.rs:27`), because the agent's `tests/mcp_server.rs` and the eval binary both need it and a `cfg(test)` module is invisible across a crate boundary. It is the **one** generator: the media fixtures, the agent end-to-end tests, and `color-workflow-v6`'s fixture builders all call it, so a raster cannot drift between the three claims made about it.

**A11 — the module doc states the boundary.** `cc7_sources` is **test support**: it shells out to the provisioned CLI through `run_ffmpeg`, and `run_ffmpeg` **panics** on a missing binary and on a nonzero exit (`test_support.rs:279-297`). The module doc says so in the same words `test_support`'s does, so nothing in production ever reaches for it. Probe-1 confirmed the synthesis path has no `cfg(test)`-only dependency: `run_ffmpeg` uses only `std::process::Command`, `std::fs`, and `std::env`.

### 3.1 Public functions

```rust
pub fn cc7_base_scene_rgb(x: u32, y: u32) -> [u8; 3];                        // cam A, §2.3.3
pub fn cc7_camera_scene_rgb(camera: Cc7Camera, x: u32, y: u32) -> [u8; 3];   // §2.4.3, in linear
pub fn cc7_log_scene_rgb(x: u32, y: u32) -> [u8; 3];                         // §2.4.2
pub fn cc7_tracked_scene_rgb(x: u32, y: u32, frame: u32) -> [u8; 3];         // §2.3.6

pub fn cc7_camera_source(camera: Cc7Camera) -> GeneratedMedia;               // 60 frames
pub fn cc7_log_source() -> GeneratedMedia;                                   // 60 frames, BT.709 TAGGED
pub fn cc7_tracked_source() -> GeneratedMedia;                               // 100 frames
pub fn cc7_scenario_sources(scenario: Cc7Scenario) -> Vec<GeneratedMedia>;

pub fn log_like_inverse_cube(size: u32) -> String;                           // the `.cube` text
pub fn write_log_like_inverse_cube(directory: &Path, size: u32) -> PathBuf;
```

### 3.2 The mux recipe, normative

Identical to CC6's (`cc6_fixtures.rs:555-596`) because CC1 rejects an untagged source and FFV1's losslessness is already proven in-suite by `verify_native_ramp` (`cc1_fixtures.rs:509-541`):

1. Compute each frame's `R'G'B'` in Rust from the functions above.
2. Convert to `yuv444p` planes with an **independently transcribed** f64 limited-range BT.709 forward matrix (`KR 0.2126`, `KB 0.0722`, `CB 1.8556`, `CR 1.5748`, `16 + 219·Y`, `128 + 224·C`). Rule 11.0.1's transcription clause permits this for *source content*; nothing compares a measurement against it.
3. Write all frames to a temp `.yuv` (`run_ffmpeg` cannot pipe stdin).
4. Invoke the pinned CLI with, verbatim:

```text
-f rawvideo -pix_fmt yuv444p -s 320x180 -r 25 -i <temp.yuv>
-vf setparams=range=limited:color_primaries=bt709:color_trc=bt709:colorspace=bt709
-c:v ffv1 -level 3 -g 1 -pix_fmt yuv444p
-color_primaries bt709 -color_trc bt709 -colorspace bt709 -color_range tv
```

output `.mkv` through `GeneratedMedia::ffmpeg(label, &arguments, "mkv")`, written to `std::env::temp_dir()` and deleted on `Drop`. **No fixture bytes are checked in** (`docs/MEDIA-POLICY.md`); the only checked-in CC7 artefacts are `cc7_manifest.json` and the three `benchmarks/auto-edit/v6/` files.

**lavfi authors nothing that carries an expectation.** `geq`/`lutrgb` floor rather than round and 8→16-bit promotion through swscale is not `×257` (measured `32790`), so every CC7 raster is idiom A.

### 3.3 The log-like carrier

The `(c)` clip is **BT.709-tagged**. Log tags stay refused: `classify_source` accepts only `Srgb | Bt709 | Bt1886` (`color.rs:730-739`) and `open_scaled_managed` blocks managed decode for `Log`/`LogC`/`Log3G10` (`decode.rs:1092-1097`), asserted by the existing `cc1_fixtures.rs:3139/3146/3153` and `:3294-3296`. **CC7 cites those fixtures; it does not duplicate them and does not amend CC1.** The carrier's *content* is log-ish and is undone by a node — that is the whole scenario.

### 3.4 `log_like_inverse_cube(size)` (A2)

A `.cube` whose output for lattice input `e ∈ [0, 1]` is `clamp(encode_bt709(2^(12e − 8)), 0, 1)`, written with `LUT_3D_SIZE size`, `DOMAIN_MIN 0 0 0`, `DOMAIN_MAX 1 1 1`, and 6-decimal fixed formatting, identical on all three channels. It is bound at `input_encoding_token = 0` (`Display709`, `color_pipeline.rs:1286-1293`), so its input is `e = encode_bt709(x)` and the production path is `z = Lut3d::lookup(e)` (**tetrahedral**, `color_pipeline.rs:1429-1489`) then `x' = decode_display709(z)`.

**The output clamp is kept.** A `.cube` whose domain is `[0, 1]` and whose outputs leave it is not a well-formed cube, and CC7 does not author one to buy four codes.

**`CC7_LOG_CUBE_SIZE = 65` is pinned, and the sweep is evidence rather than a selection rule (A22).** Probe-3 measured the set-wide worst monitoring error over A2's gate set — the twelve achromatic chart patches plus the four skin patches — at **13 / 7 / 4** codes for lattice sizes **17 / 33 / 65**. Read as a *rule*, "the smallest size within `CC7_LOG_INVERSE_MAX_CODE = 12`" would select **33**, not 65 (E16); the contract therefore **pins the size** and requires the sweep to be **monotone non-increasing with `17 > 12 ≥ 33 > 65`**, so size 17 genuinely fails and the sweep is not vacuous. The measured file is **`CC7_LOG_CUBE_BYTES_REPORTED = 7 414 990` bytes**, **44.2 %** of `LUT_MAX_FILE_BYTES = 16 MiB` (`lut_store.rs:33-42`); `129³` would be ≈ 58 MB and would not fit. The extra 6.4 MB of `.cube` text over size 33 buys a margin of **3.0×** instead of 1.7×, and probe-3 measured the whole (c) section — two encodes, two managed decodes, a GPU proof pair and four cubes — at **0.99 s**.

Two error floors are structural and do not shrink with size, and the contract names both: **black** (`CC7_LOG_BLACK_PATCH_REPORTED_CODE = 4` at 17, 33 **and** 65 — the curve is not invertible at `v = 0`, §2.4.2) and **the saturated primaries** (`CC7_LOG_PRIMARY_REPORTED_CODE = 5` at size 65 — a sub-percent `e` error amplified through the exponential). A1 moved the primaries into their own band and A2 excludes them from the gate set, so the gate does not see the second floor; it is recorded, not gated.

### 3.5 Non-vacuity rules

Each is a `cc7_` fixture; a source that fails one makes every claim measured on it meaningless, so these run before the gates that consume them.

1. **`cc7_camera_sources_differ_from_the_reference_at_every_neutral_patch`** — for each of B, C1, C2, at least one channel of each of the **twelve** achromatic chart patches differs from cam A's code, and the whole-raster mean absolute code difference against cam A is ≥ 5 codes.
2. **`cc7_log_source_is_not_the_base_scene`** — the log clip's chart-patch codes equal §2.4.2's stored log codes exactly; its monitoring luma first percentile is ≥ `CC7_LOG_FIRST_PERCENTILE_MIN_CODE` and its 99th ≤ `CC7_LOG_P99_MAX_CODE`, while cam A's fail both.
3. **`cc7_tracked_source_moves_and_occludes`** — at each of the eleven `CC7_TRACK_SAMPLE_FRAMES` the pixel at the analytic centre is the `product_red` code except at frame **47**, where it is `CC7_SURROUND_CODE`; consecutive sampled centres differ by ≥ 1 px; the square is fully inside the raster at every frame `0..99`.
4. **`cc7_tracked_square_never_covers_the_static_patch_row`** — §2.3.6's generator assertions (`y ≥ 24` against the patch rows at `y 4..20`) over all 100 frames.
5. **`cc7_ffv1_round_trip_is_byte_exact`** — one generated `.mkv` per generator re-decoded and compared byte-exact against the authored `yuv444p` planes, in `verify_native_ramp`'s shape.
6. **`cc7_base_scene_populations_are_the_contract_table`** — §2.3.3's five population counts and their sum, `57 600`.
7. **`cc7_the_chart_band_is_achromatic_and_the_primaries_band_has_no_red`** (A1) — every chart patch satisfies `R == G == B`; the primaries band contains exactly five patches and **no** `[255, 0, 0]`; and the derived `product_red` qualifier's hue centre is more than `MATTE_SAMPLE_HUE_WIDTH_CENTIDEGREES + MATTE_SAMPLE_SOFTNESS` from every primary patch's grade709 hue. This is the fixture that stops a later "tidy-up" from putting the red primary back and silently breaking (d).

---

## 4. Technical gates

Ordinary `cargo test`, default lane, both CI operating systems, **no model, no network, nothing environment-gated**. GPU work runs on `fallback_gpu()` (`cc1_fixtures.rs:1494-1520`), which fails loudly, with an `#[ignore]` `hardware_gpu()` twin for the parity gates. `fixture_gpu_or_skip` and `KINEWRIGHT_GPU_TESTS_MAY_SKIP` are **forbidden** in every file named in this section, guarded by `uses_outside_prose` (§11.3).

Every item states the **measuring function**, the **sampling rule**, the **passing gate**, and the **failing-direction fixture**. Patch statistics are taken on a **2-pixel inset** of each patch rect.

**Crate attribution, once, for every bare path in this section and in §2 (R-M1):** `color_scopes.rs`, `color_status.rs`, `server.rs`, `schema.rs`, `runtime.rs`, `eval.rs` and `export_queue.rs` are **`crates/kinewright-agent/src/`**. `color_qc.rs`, `scopes.rs`, `delivery.rs`, `effect.rs`, `operation.rs`, `media.rs` (including `matte_coverage_statistics` and `MatteCoverageStatistics`, `:673-726`) and `color.rs` are **`crates/kinewright-core/src/`**. `color_pipeline.rs`, `compositor.rs`, `verify.rs`, `export.rs`, `lut_store.rs`, `builtin_looks.rs`, `test_support.rs` and every `ccN_fixtures.rs` are **`crates/kinewright-media/src/`**. `inspector_ui.rs`, `look_browser_ui.rs`, `media_workflow.rs` and `app.rs` are **`crates/kinewright-app/src/`**. A gate that names a symbol must be reachable from the crate the fixture lives in; §2.7 and §11.0.1 state the one place where it is not, and what CC7 does about it.

### 4(a) Mixed-camera interview

1. **Reference retention.** *Measures:* the `Document` re-read through `query_document` after commit. *Sampling:* the whole document. *Passes:* clip 1 carries **zero** effects and its serialized JSON is byte-identical to its pre-commit form, and no operation in the returned batch names clip 1 (`crates/kinewright-agent/src/color_scopes.rs:788-869`). **The effect-count check is the evidence.** `plan_shot_match`'s `reference_retained` is a hardcoded `true` literal (`color_scopes.rs:906`) — it asserts nothing about what happened and CC7 **does not cite it as evidence of retention** (R-M19); the fixture may assert it is present, but never as the proof. This gate is structural, not a budget. *Fails:* `cc7_a_reference_retention_fails_when_the_reference_is_also_a_candidate` — clip 1 as both reference and candidate is refused `invalid_request` "reference shot must not also be a candidate" (`color_scopes.rs:793-796`).
2. **Post-match neutral spread (A1, A8).** *Measures:* `Analysis::monitor_proof_for_document` on the committed document, read at the **twelve** achromatic chart-patch rects of clip 2's frame. *Sampling:* the twelve rects at project frame 60, 2-px inset. *Passes:* `max over the twelve patches of max(|R−G|, |G−B|) ≤ CC7_MATCH_NEUTRAL_SPREAD_MAX_CODE = 5`. **Measured 2** (P2, worst patch 2 = code 24), **2.5×** margin. Cam A's own chart band measures spread **0**. **The budget is 5, not 6** (A15): probe-2 measured the *unmatched* cam B at **exactly 6**, so a `≤ 6` gate would have passed its own failing-direction fixture. *Fails, two directions:* `cc7_a_the_unmatched_candidate_exceeds_the_neutral_spread_budget` — unmatched cam B at **6** — and `cc7_a_the_unrecoverable_candidate_exceeds_the_neutral_spread_budget` — corrected C2 at **19** (`CC7_UNRECOVERABLE_RESIDUAL_SPREAD_REPORTED_CODE`, worst patch 11 = code 255).
3. **Post-match luma mean (A8).** *Measures:* the same monitor proof; `mean(luma_B) − mean(luma_A)` over the chart band ROI `(0, 2000, 3000, 888)`, in code millionths (`round(v · 1_000_000)`, half away from zero). *Sampling:* the chart band on clip 1 frame 0 and clip 2 frame 60. *Passes:* `|Δ| ≤ CC7_MATCH_LUMA_MEAN_MAX_CODE_MILLIONTHS = 5_000_000`. **Measured −1 381 567** (P2), **3.62×** margin. *Fails:* `cc7_a_the_unmatched_candidate_exceeds_the_luma_mean_budget` — unmatched cam B measures **−19 904 917**, 3.98× over. Corrected C2 measures **−4 302 267** and *passes* this term, which is why the spread and the luma mean are two gates and not one.
4. **Intentional difference survives.** *Measures:* the `plan_shot_match` response and `get_color_qc`. *Passes:* `saturation_percent` appears in **no** proposed operation and in no `proposal_details` key (`color_scopes.rs:1848-1852`); clip 2's skin ROI `(0, 4223, 1500, 888)` reports `in_band_basis_points == CC7_SKIN_IN_BAND_EXACT_BASIS_POINTS` (**measured exactly 10 000** on both cam A and matched cam B, P1; re-measured at §12 step 5); and the skin-row chroma spread is asserted **smaller** on the matched candidate than on the reference (measured 52.75 against 60.50 codes, P1; re-measured at §12 step 5), which is the intentional desaturation surviving the match rather than being corrected away. *Fails:* `cc7_a_skin_band_rejects_the_product_row` — the same measurement over the `product_red` patch reports `in_band_basis_points == 0` and a `skin_region_outside_band` **Info** exception (`color_qc.rs:1251-1268`).
5. **Render parity (A5).** *Measures:* the **existing** `LinearParityMetrics` / `assert_linear_parity` banded gate (`cc1_fixtures.rs:894-926`) applied **at the compositor layer** — `Compositor::render_working` against the CPU reference over a `WorkingFrame` built from the CC7 raster carrying the canonical (a) node stack — exactly as `cc3_fixtures.rs:517` and `cc4_fixtures.rs:898` do. `LINEAR_CPU_GPU_MAX 1.5e-3`, `P99 7.5e-4`, `MEAN 2.5e-4`, over-range `9.765625e-4`, **reused unchanged**; no document-level parity helper exists and CC7 does not invent one. *Additionally recorded as evidence, not as a budget:* two renders of the canonical document's working surface on one lane agree at max/P99/mean **`0 / 0 / 0`** over **172 800** samples with **0** non-finite (P1). *Fails:* the CC6 negative control — the same comparison against a reference perturbed by `2 × LINEAR_CPU_GPU_MAX`.

### 4(b) Wrong white balance and underexposure

1. **(b1) recoverable.** *Measures:* §4(a)(2)'s spread on the committed C1 document. *Passes:* `≤ CC7_B1_RESIDUAL_SPREAD_MAX_CODE = 6` (**superseded by §0.3 G-E1**: this text originally reused the (a) budget of 5; the amended scene measures **3** against 7 uncorrected, a 2.0× margin, and 5 would have been a 1.67× margin). **The absent-key rule is normative (R-M19):** a control whose rounded delta is `0`, or whose ratio is non-finite, is **omitted entirely** from `proposal_details` (`color_scopes.rs:1897-1903`) — an absent key means *not proposed*, never *zero*. The gate therefore iterates the **present** controls and asserts `clamped == false` on each, **and additionally asserts that `temperature_percent` IS present**, so a run in which the planner proposed nothing at all cannot pass by vacuous iteration. `PlanShotMatchArgs.roi` is a **single shared field** (`color_scopes.rs:266-267`) applied to the reference and to every candidate (`:801`); there is no per-clip ROI, and the (a)/(b) scripts pass the achromatic chart band once. *Fails:* (b2)'s own measurement, item 2.
2. **(b2) beyond authority.** *Measures:* the `plan_shot_match` response for C2. *Passes:* `temperature_percent.clamped == true` with `min == -100`, `max == 100`, and an `unrounded_delta` of **+248** rounded into the published `requested` (P2); `exposure_milli_stops == 2410` with `clamped == false`. The residual spread is **reported, never gated**: `CC7_UNRECOVERABLE_RESIDUAL_SPREAD_REPORTED_CODE = 19` — that residual *is* the compromise the human is asked about, and it is also §4(a)(2)'s second failing direction. *Fails:* `cc7_b_c1_publishes_no_clamp` — the C1 response has `clamped == false` on all three controls, so the clamp assertion is not tautological.
3. **(b2) the compromise is visible and typed.** *Measures:* `get_color_qc` on the corrected C2 clip with `checks: ["range", "gamut", "tags", "per_node"]`, `max_nodes: 16`. *Sampling:* clip 2's first frame, whole raster. *Passes:* **exactly one** `delivery_range_excursion` **Warning** is present, on `field = "blue.over_basis_points"`, with `observed == 116` basis points against `allowed "< 10"` (11.6× the threshold) and `technical_pass == true`, because a Warning is not an Error. **The over-range population is 672 pixels on the blue channel only** (`CC7_C2_OVER_RANGE_PIXELS_REPORTED` / `..._BASIS_POINTS_REPORTED`), `red.over_pixel_count == 0`, `green.over_pixel_count == 0`, `blue.maximum_over_excursion_millionths == 41 538`; it comprises **the blue, cyan and magenta primary patches (384 px), the white achromatic patch (128 px), and the ramp's brightest columns (160 px)** (A16, P2). Draft v2's "the blue primary clips, not the whites" was measured on the pre-amendment scene and is **superseded**: A1 added a 255 achromatic patch, and at this exposure the whites clip too. Per-node attribution names the `primary_correction` node as the sole cause with `gamut_basis_points_delta == 0` and `attribution == "node_removed"`; the `range_basis_points_delta` is expected to be **+116** by the same mechanism probe-1 measured as `+22` on the old scene, and the implementer **confirms it at §12 step 5 rather than pinning it from here** (A20). The corrected clip's skin `in_band_basis_points` measures **9 411** (`CC7_C2_SKIN_IN_BAND_REPORTED_BASIS_POINTS`, P1; re-measured at §12 step 5), still above `SKIN_BAND_EXCEPTION_BASIS_POINTS = 5_000`, so no Info exception fires and the compromise is visible in the number without a new code. *Fails:* `cc7_b_c1_raises_no_range_excursion` — the corrected **cam B** control measures `clamped_basis_points == 0`, no exceptions, and per-node deltas `0 / 0` (P2), so neither the excursion nor the attribution is vacuous.
4. **Noise is not measured.** Deferred with its reason in §13; no gate here reads a noise term, and none exists to read.

### 4(c) Log-like input

1. **The signature is agent-readable, in the unit the tool publishes (A9, A21).** *Measures:* `analyze_color_shot` → `scope_statistics.luma.first_percentile` and `.ninety_ninth_percentile` (`color_scopes.rs:1712`, serializing `ScopeEvidence::statistics` verbatim). **These fields are 16-bit full-scale codes — the 8-bit value × 257** (`scopes.rs:576-586`, produced at `:1330-1339` where `percentile_code` returns `value * 257`). **`mean_code_values.luma` is the wrong field**: it is the 8-bit `integer_luma_code` *mean* (`color_scopes.rs:1704`), not a percentile, and there is no 8-bit percentile anywhere in the payload. The contract names it so no implementer compares an 8-bit constant against a 16-bit JSON number. *Sampling:* the clip midpoint, full raster, `include_grids: false`; percentiles by the existing nearest-rank convention `ceil(p·N/100)` (`scopes.rs:1319`) over all 57 600 pixels. *Passes:* `first_percentile ≥ CC7_LOG_FIRST_PERCENTILE_MIN_CODE16 = 5_140` **and** `ninety_ninth_percentile ≤ CC7_LOG_P99_MAX_CODE16 = 51_400`. **Measured on the amended scene (P3, probe-3): carrier `7 196 / 31 611 / 42 919`** against **cam A `2 570 / 29 555 / 62 194`** — 2 056 codes of headroom below the carrier's p1 and 8 481 above its p99, with cam A 2 570 below and 10 794 above on the failing side. In 8-bit prose those are **carrier 28 / 123 / 167 against cam A 10 / 115 / 242**, and the manifest records `20` / `200` as prose equivalents only; the **gated** numbers are the 16-bit pair. Probe-3 measured the CPU reference on the decoded working frame and the real `monitor_proof_for_document` GPU raster on the lavapipe lane as **byte-identical**. *Fails:* `cc7_c_the_base_scene_does_not_read_as_log` — cam A fails **both** bounds, `2 570 < 5 140` and `62 194 > 51 400`. **Had the constants been left in 8-bit**, the p1 gate would have passed on every source and the p99 gate failed on every source; that is why A21 restates the unit rather than the number.
2. **The inverse lands (A2).** *Measures:* `Analysis::monitor_proof_for_document` on the committed document (imported `.cube` → `technical_lut` at the input stage, mix 1.0, `input_encoding_token = 0`), read at the twelve achromatic chart rects and the four skin rects. *Sampling:* project frame 0, 2-px inset. **The gate is the set-wide worst**, over the twelve achromatic chart patches **and** the four skin patches, never a single patch: probe-3 measured `chart06` (code 128) at **1** code under an *identity* cube, because the log curve is near-neutral at mid-grey, so a single-patch gate at 128 would be vacuous. *Passes:* the worst absolute monitoring-code error over the gate set — max over pixels and channels on a 2-px inset of each patch — is within `CC7_LOG_INVERSE_MAX_CODE = 12`. **Measured 4 at size 65** (P3, probe-3, on the amended scene), a **3.0×** margin, with the worst landing simultaneously on `chart00` (black), `chart11` (white) and `skin_light`. The six intermediate greys A1 added are the **best-behaved patches in the set** at 0–2 codes at every size, so draft v2's stated risk that they sat where the exponential amplification is largest is **refuted by measurement**. `product_red` measures 1, `product_cyan` 3 and `deep_shadow` 0; the five primaries measure 5 and are excluded from the gate set by A2. *Fails:* `cc7_c_an_identity_cube_does_not_undo_the_log_curve` — an identity 33³ cube measures **`CC7_LOG_IDENTITY_CUBE_REPORTED_CODE = 85`** over the gate set (`chart11`; `skin_light` 56), a **7.1×** overshoot of the budget and **21×** the passing figure. Note the one inversion the contract must state plainly: under the identity cube `chart00` reads **0** while under the correct one it reads **4**, because the black patch's error is a property of the curve (§2.4.2) and not slack in the LUT.
3. **The lattice sweep is evidence, and the size is pinned (A22).** `cc7_c_the_cube_size_sweep_is_monotone_and_size_seventeen_fails` asserts the set-wide worst at **17 = 13**, **33 = 7**, **65 = 4** (P3, probe-3, amended scene), that the sequence is **monotone non-increasing**, and that `17 > CC7_LOG_INVERSE_MAX_CODE ≥ 33 > 65` — so size 17 genuinely fails the budget and the sweep is not vacuous. **`CC7_LOG_CUBE_SIZE = 65` is pinned rather than selected** (§3.4): read as a selection rule the sweep would choose 33, at a 1.7× margin, and the programme's bar is 2×. The black patch's **4** codes are asserted **size-independent** at all three sizes, which is what makes it a property of the curve rather than of the lattice. `CC7_LOG_CUBE_BYTES_REPORTED = 7 414 990` is asserted under `LUT_MAX_FILE_BYTES`.
4. **Node order.** *Measures:* `get_color_context`'s `color_nodes` manifest. *Passes:* the `technical_lut` node's `color_stage` is `input` and its index precedes every `correction`- and `look`-stage node (`effect.rs:1859-1866`, `color_status.rs:3959-3976`). *Fails:* covered by the existing `the_insert_index_puts_a_technical_lut_before_every_correction` (`inspector_ui.rs:5673`), named in the inventory rather than duplicated.
5. **The import needs a saved project (A9).** `import_lut_asset` returns `project_not_saved` (`server.rs:352`, described `:9662`) until the branch server carries a project-path handle, and `AddLutAsset` is refused on every other path (`server.rs:1086-1090`, `:1404`, asserted at `tests/mcp_server.rs:1168-1180`). The (c) agent test **copies** `cc4_branch_server_with_the_project_path_handle_resolves_imported_availability` (`tests/mcp_server.rs:1351`), with `cc4_render_color_proof_reports_the_unpublished_lut_asset_from_the_real_renderer` (`:1439`) as the proof-side sibling.
6. **The refusal is unchanged.** The log-tagged refusals stay CC1's; the manifest cites `cc1_fixtures.rs:3139/3146/3153` and `:3294-3296` in `external_owners` and CC7 adds no assertion of its own about them.
7. **No human question.** The matrix has no row for log-like input; scenario (c) is objective-only and contributes **no** entry to the blind package.

### 4(d) Product and skin

1. **Qualifier containment, exact (A1).** *Measures:* `inspect_grade_matte` on the committed **(d)** node (qualifier-only, no window) and `MatteCoverageStatistics` (**`crates/kinewright-core/src/media.rs:673-726`** — core, not media) from `Analysis::matte_proof_for_document`. **`matte_coverage_statistics(coverage: &RgbaImage)` takes one argument, measures the whole raster, and rates every count over `total_pixel_count = w·h` (`kinewright-core/src/media.rs:724-736`); it has no ROI parameter** (R-M6), so a fixture that wants a region **crops the coverage raster to that region first** and the contract says so rather than implying an argument that does not exist. The 16-bucket field is `coverage_histogram`, not `histogram`. *Sampling:* project frame 0. *Passes:* `covered_pixel_count == full_pixel_count == CC7_PRODUCT_PATCH_PIXEL_COUNT` (**192**) and `partial_pixel_count == 0`, exactly — CC5's precedent (`cc5_fixtures.rs:4725-4756`), no tolerance. **Measured on the amended scene: `covered 192 / full 192 / partial 0`, `covered_basis_points 33`, coverage by region `{product_red: 192}` and nothing else** (P2). This is reachable **only** because A1 removed the red primary: on the pre-A1 scene probe-1 measured `covered 320 / full 192 / partial 128`, the 128 intruders being the red primary at grade709 hue 0 cd, 135 cd from the derived centre. Neither the magenta primary (30 000 cd) nor the yellow (6 000 cd) is caught, exactly as A1 predicted — both are more than `1 500 + 1 000` cd of hue softness away. *Fails:* `cc7_d_a_qualifier_that_selects_two_patches_is_rejected` — widening `matte_hue_width_centidegrees` to 18 000 (the neutral, which disables the hue leg) selects more than 192 pixels.
2. **Media containment.** *Measures:* the existing `assert_matte_containment` (`cc5_fixtures.rs:1149-1172`). *Passes:* `outside_changed_pixels == CC7_MATTE_OUTSIDE_CHANGED_PIXELS_MAX` (**0**, "no tolerance may excuse one") and inside changed ≥ `MIN_CHANGED_LINEAR_BASIS_POINTS = 500`; probe-2 measured **192 changed inside and 0 outside** on the amended scene (the outside map is empty); probe-1 measured **128 outside** on the pre-A1 scene, which is precisely what A1 removes. *Fails:* the same helper against a window deliberately one patch to the left.
3. **Hue stability.** *Measures:* `get_color_qc` `checks: ["skin"]`, `roi = (0, 4223, 1500, 888)`. *Sampling:* project frame 0, before and after the commit. *Passes:* `mean_hue_centidegrees` is `Some(_)` on **both** sides and the two are **equal** (delta exactly `0`) — `SkinDiagnostics.mean_hue_centidegrees` is `Option<i32>` (`crates/kinewright-core/src/color_qc.rs:437-460`) and a `None` on either side **fails the gate**, it is not a pass by default (R-M5). `in_band_basis_points == CC7_SKIN_IN_BAND_EXACT_BASIS_POINTS` (10 000) on both sides, and the contract states what that rate is over: **the considered (non-achromatic) pixels**, not every pixel of the region (`color_qc.rs:453-456`). `excluded_achromatic_pixel_count` and `considered_pixel_count` are **reported** beside it, so "10 000" can never be read as a claim about a population it did not measure. *Fails:* `cc7_d_a_qualifier_over_the_skin_row_moves_the_skin_hue` — the same node with the qualifier derived from `skin_tan` moves `mean_hue_centidegrees` by a non-zero amount.
4. **(d2) matte edges (A7).** *Measures:* `MatteCoverageStatistics.{full,partial,covered}_pixel_count`. *Sampling:* project frame 0. The node is **window-only** (R-B4, §2.5): the qualifier is off, so `Matte::coverage`'s product (`color_pipeline.rs:2109-2112`) is the window leg alone and the numbers below are the window's. Feather scales the **normalized distance field**, not the raster: `w = 1 − smoothstep(1−f, 1+f, D)` (`color_pipeline.rs:1798-1832`), so the band is a fraction of the window's own half-extents — a 1 000 bp feather on a 6 × 8 px half-extent window is a ~1.2 px band. **`D = max(|u−cx|/hw, |v−cy|/hh)` is the `Rect` branch only**, it omits the rotation term, and it does not apply at all when `feather <= 0`, where the function takes a hard `D <= 1.0` step (minor 3). (d2) pins `shape_token = 1` and rotation `0`, which is why the stated form is the function *here*; the contract does not present it as the general one. **The analytic model is the discrete pixel-centre count:**

```text
inner   = #{(x,y) : |x+0.5 − cx_px| <= (1−f)·hw_px  and  |y+0.5 − cy_px| <= (1−f)·hh_px}
outer   = #{(x,y) : |x+0.5 − cx_px| <  (1+f)·hw_px  and  |y+0.5 − cy_px| <  (1+f)·hh_px}
partial = outer − inner
```

   With `cx_px = 53.984`, `cy_px = 83.988`, `hw_px = 5.984`, `hh_px = 7.992`, `f = 0.1`: inner = 10 × 14 = **140**, outer = 14 × 18 = **252**, partial = **112**. All three matched the measurement **exactly** (P5, re-derive if the patch moves). **The continuous-area formula `4·hw·hh·((1+f)² − (1−f)²) = 76.8` is the wrong model** — it is wrong by 35 pixels on this window (31 %) — and the contract names it so no reader re-derives it. *Passes:* `|full − 140| ≤ CC7_FEATHER_PARTIAL_TOLERANCE_PIXELS`, `|covered − 252| ≤ …`, `|partial − 112| ≤ …`, with the tolerance **4** absorbing the basis-point quantization of `cx/cy/hw/hh` at a boundary (measured error against the discrete model: **0**). *Fails:* `cc7_d_feather_zero_has_no_partial_pixels` — the same window at `feather = 0` reports `covered == full == 192`, `partial == 0`, and exact 0/255 coverage (`compositor.rs:6159-6197`). **(d2) is NOT cuttable** (R-M16): it is the roadmap matrix's "matte edges" objective check on the Product-and-skin row, and cutting it would leave a required cell undelivered. §12's cut order names two other things.

### 4(e) Creative look

1. **Bypass is the lossless twin of absent (R-M4).** *Measures:* `render_color_proof` with `effect_id` and `look_comparison` in `before`, `after`, `bypass`. *Passes:* the key **`look_comparison.bypass_matches_absent`** — it is nested, `server.rs:3259`, produced at `:3046-3067`, and **absent whenever no stored node was proofed** — is present and `true`, and `hashes.before_rgba8_pixels_sha256 == hashes.after_rgba8_pixels_sha256` on the `bypass` call. *Fails:* **the typed refusal `bypass_not_lossless`** (`server.rs:3050-3064`, `color_status.rs:480`/`:539`), because the value is only ever `true` — a mismatch is not reported as `false`, it is refused. The construction that reaches it is a node whose bypass cannot be applied losslessly; **the implementer verifies which construction the code actually refuses** (a keyframed `bypass`, or a look node the bypass path declines) and names it in the fixture. **If no construction is reachable**, the failing direction is instead the hash-equality check on the **`after`** call — `after_rgba8_pixels_sha256 != before_rgba8_pixels_sha256` — and the contract records that the typed refusal is unreachable rather than pretending the gate has a failing direction it does not have.
2. **Gamut, exactly where it is analytic (A3).** *Measures:* `get_color_qc` `checks: ["gamut", "range"]`, `ColorGamutReport` (`color_qc.rs:360-372`, definition `min(r,g,b) < 0` in linear light). The look is `BuiltinLook::Warm` baked at size 17 over domain `[−1, 2]`, bound at `LutInputEncoding::Display709` (token 0, the descriptor neutral — exactly what `plan_creative_look` with no `input_encoding_token` produces), mix 1.0; the formula is per-channel affine, so the bake reproduces it essentially exactly (P4). *Sampling:* the `deep_shadow` ROI `(2250, 4223, 375, 888)` for the gate — the `ceil`ed `y` is normative (A19); the naive `4222` resolves to `y 75, h 17` = 204 pixels — and the whole raster for the report. *Passes:* `out_of_gamut_pixel_count == CC7_LOOK_DEEP_SHADOW_OUT_OF_GAMUT_PIXELS == 192` **exactly** (12 × 16, zero decode sensitivity), and `delivery_gamut_excursion` is present with severity **`Warning`** and `technical_pass == true`. **Measured on the amended scene** (P2): ROI count **192** with `out_of_gamut_basis_points 9 411`, `below_black_pixel_count 0`, `minimum_linear_millionths −5 722`. **Reported, never gated:** the whole-raster `out_of_gamut_pixel_count` **1 480** and `out_of_gamut_basis_points` **256** (`CC7_WARM_WHOLE_RASTER_OUT_OF_GAMUT_*`; 1 608 / 279 on the pre-A1 scene — the 128-pixel difference is exactly the removed pure-red patch, whose green *and* blue both go negative), `below_black_pixel_count 348`, `minimum_linear_millionths −17 776`, `maximum_desaturation_millionths 996 991`, and the five accompanying whole-raster `delivery_range_excursion` Warnings (red over 172, green over 130 / under 111, blue over 44 / under 212). **The whole-raster count is not analytic**: probe-1 measured 1 608 against 1 568 predicted from source codes, and the 40-pixel gap is exactly two ramp columns whose codes sit within one code of the `e = 0.074 074 1` threshold and are moved by the limited→full decode round trip. On the ROI there is one accompanying `delivery_range_excursion` (blue under, 9 411). *Fails:* `cc7_e_the_base_scene_without_the_look_is_in_gamut` — the same measurement with the look removed reports `out_of_gamut_pixel_count == 0` on both the ROI and the whole raster, and no Warning.
   Probe-2 confirmed the look node's binding: `BuiltinLook::Warm.to_lut_asset(LutAssetId(1))` as a `creative_look` with `input_encoding_token = 0` and `mix_basis_points = 10 000` — exactly what `plan_creative_look` with no token produces.
3. **Ordering with (c) present.** *Measures:* `get_color_context`'s `color_nodes`. *Passes:* with the (c) `technical_lut` **and** the (e) `creative_look` on one clip, the manifest lists the input stage strictly before the look stage. *Fails:* `stage_insert_index` makes an ordering rejection unreachable constructively, so the failing direction is a hand-built document whose nodes are in the wrong order, asserted rejected by `validate_document` with `ColorStageOrderViolation`.
4. **Portability.** The gate is CC4's existing bit-identical relocation fixture `cc4_relocating_the_store_reproduces_the_render_bit_identically` (`cc4_fixtures.rs:3516`), **cited, not duplicated**. CC7 adds exactly one agent-visible check: after a Save-As relocation, `list_look_assets` reports the (c) imported asset `verified` with the same `sha256`. *Fails:* `cc7_e_a_bare_relocation_reports_missing`. **This check is NOT cuttable** (R-M16): it is the matrix's "asset portability" objective check on the Creative-look row.

### 4(f) Tracked secondary — measured (A12–A14, A17, A18)

**Every tracking gate reads `observations[]` (raw), never `curves`** (A17). The smoothed curve is a three-sample median filter plus a step clamp with the tool's own published `window_stabilization.known_systematic_lag`; probe-2 measured its final keyframe **746 bp** off whenever the last sample is dropped, for a documented reason that has nothing to do with tracking quality. A gate written against `curves` would fail on the smoother.

1. **Low-confidence identity (A12, A14).** *Measures:* `track_matte_window`'s `low_confidence_samples` and `observations` (`server.rs:4667-4713`). *Sampling:* `start_local_frame = 0, end_local_frame = 48, step_frames = 5, search_radius_percent = 10, max_width = 256`, giving `CC7_TRACK_SAMPLE_FRAMES` (11 samples, §2.3.6), with `minimum_confidence_basis_points = CC7_TRACK_MIN_CONFIDENCE_BASIS_POINTS = 8_500`. *Passes:* `low_confidence_samples` local frames == exactly **`{47}`**, and the other ten samples all survive. The floor sits between the measured **occluded maximum 7 411 bp** (frame 47 on this recipe reads **7 349**) and the measured **clean minimum 9 740 bp** (frames 32 and 37) — **+1 089 / −1 240 bp**, both sides above A4's 1 000 bp bar, on a 2 329 bp separation. **The `DEFAULT_MATTE_TRACK_MINIMUM_CONFIDENCE_BASIS_POINTS = 5 000` drops nothing** (measured `low_confidence_samples == []` at 5 000 and at 7 000), so the default must not be reused and `CC7_TRACK_MIN_CONFIDENCE_BASIS_POINTS != 5_000` is asserted. *Fails:* `cc7_f_the_default_floor_drops_no_sample` — the identical call at 5 000 returns an **empty** `low_confidence_samples`, so the floor is load-bearing and the gate is not satisfied by the tool's default.
2. **Observation accuracy (A14, A17).** *Measures:* `observations[].center_x_basis_points` / `center_y_basis_points`, raw and pre-stabilization. *Passes:* every **surviving** observation is within `CC7_TRACK_TOLERANCE_BASIS_POINTS = 200` of §2.3.6's analytic centre. **Measured worst 49 bp** (y, frames 14 / 18 / 32) — a **4.08×** margin — reusing CC5's `matte_track_tolerance_basis_points = 200` unchanged rather than inventing a CC7 value. *Fails:* `cc7_f_observation_gate_rejects_a_doubled_offset` — the same comparison against a centre offset by `2 × CC7_TRACK_TOLERANCE_BASIS_POINTS`.
3. **Matte containment over time (A17).** *Measures:* the committed window's resolved rect against the analytic square, at every **surviving** sample **except the final keyframe, frame 42**, which is named and excluded because it carries the tool's `known_systematic_lag`. *Passes:* the **1.5× window** `half_width = 563`, `half_height = 1_000` bp (18 px) contains the whole 24 × 24 square at every one. Measured worst **required** half-extents **14.77 px in x** and **12.88 px in y**, leaving **3.23 px / 5.12 px** of margin, all four recorded as reported constants (§2.3.6), not gated. *Fails:* `cc7_f_a_window_smaller_than_the_square_loses_containment` — the **seeded 1.0× window** (`375 / 667` bp, 12 px) is 2.77 px short in x and fails containment, which is the measured failing direction and not a hypothetical one.
4. **(f2) total loss is typed (A13).** *Measures:* `track_matte_window` over `start_local_frame = 0, end_local_frame = 48, step_frames = 47`, whose sample set is `{0, 47}`: sample 0 is on the square and pinned at `10 000` (`server.rs:5285-5290`), sample 47 measures **7 309**, so survivors = 1 < `MATTE_TRACK_MINIMUM_SAMPLES = 2` (`server.rs:11204`). *Passes:* the tool refuses `tracking_confidence_too_low` (`server.rs:4585`) with `field: "minimum_confidence_basis_points"`, `observed: { surviving_samples: 1, total_samples: 2, minimum_confidence_basis_points: 8500, low_confidence_samples: [{ local_frame: 47, confidence_basis_points: 7309, … }] }`, `allowed: { minimum_surviving_samples: 2 }`, the `recovery_action` string, and `evidence_only: true` / `applied: false`; the timeline revision does **not** move. *Fails, both directions:* the **same call at the 5 000 default does not refuse** (`cc7_f2_the_default_floor_does_not_refuse`) — the floor is load-bearing — and the (f)(1) full-range call succeeds one step away. **The range must start at frame 0**: probe-2 measured `43..48`, `42..48` and `39..51` all returning `10 000` on every sample and refusing nothing, because the first sample is seeded from the window's *stored static* centre (frame 0's square position) and a range starting inside the occlusion seeds the template on flat surround, which then matches flat surround perfectly.
5. **No gate spans the occlusion, and none claims the window covers a hidden subject.** Probe-2 measured the interpolated centre during frames 43..47 at the last keyframe's position, `(7 246, 6 632)` bp = `(231.87, 119.38)` px, while the hidden square sits at `(179, 90)` at frame 45 — **no overlap at all**. The honest statement, and the one the contract makes, is: *the window holds its last measured position through the occlusion, and the sample inside the occlusion is reported as low-confidence rather than as an observation.* §13 records the boundary.
6. **No track-lost marker is asserted**, because none exists (`server.rs:4617-4630`); §13 records the deferral.

### 4(g) Encoded delivery

1. **Both depths, every scenario (A6, A10).** *Measures:* `Analysis::verify_delivery_output` through the production export path on each scenario's canonical document, at `DeliveryProfile::SourceMaster`. *Sampling:* CC6 §6.2's closed-form `sample_frames`, `DELIVERY_VERIFICATION_FRAME_COUNT = 5` — `0, 14, 29, 44, 59` for a 60-frame scenario, `0, 29, 59, 89, 119` for a two-clip (a)/(b) document, `0, 24, 49, 74, 99` for (f). *Passes:* **`technical_pass == true` AND `within_budgets == true` AND every exception code ∈ `CC7_DELIVERY_ALLOWED_INFO_CODES = ["delivery_tag_not_representable"]`.** The gate is written that way because **every** verified H.264 export carries one Info `delivery_tag_not_representable` on `white_point` — the format has no white-point field — so "no exceptions" would fail a perfectly conforming encode (A6). Probed tags are `bt709 / bt709 / bt709 / limited`, depth as requested, `mismatches = []`, `conforming = true`, `decoded_pixel_format` `yuv420p` (Eight) or `yuv420p10le` (Ten). **`DeliveryVerification` has no `provenance` field** — its fields are `output_path, delivery_bit_depth, probed, tags, decoded_pixel_format, comparison, exceptions, technical_pass` (`crates/kinewright-core/src/delivery.rs:1385-1402`) — so draft v3's `provenance = StreamMetadata` claim is **dropped** (R-M7); provenance lives on the probed `ColorDescription`, and the gate asserts the tag fields it can actually read. **`Analysis::verify_delivery_output`'s trait default is `Err(MediaError::NotImplemented)`** (`crates/kinewright-core/src/media.rs:1601-1608`), so this gate holds only against the real `FfmpegMediaEngine`; the fixture constructs one and the contract says so rather than implying every `Analysis` satisfies it. **CC6's budgets are reused verbatim and CC7 never re-baselines one**; the per-scenario measured value and margin go in the manifest. *Fails:* `cc7_g_a_starved_encode_trips_the_decoded_difference_budget` — one scenario at `-b:v 100k` reports `within_budgets == false` with a `decoded_difference_over_budget` **Error**, `technical_pass == false`, and the output file still at its original path, unrenamed and undeleted.
2. **Platform consistency** is "the same gates pass in the default lane on both CI operating systems", CC6's definition unchanged (`ci.yml:10, :40`). No cross-platform decoded delta is recorded; §13 says why.
3. **The human question fires only on a `Warning` (A6).** The (g) encode enters the blind package **only** when the verification's `exceptions` contain a `Warning`-severity code — in practice `decoded_range_excursion` — while items 1's conditions still hold. An encode whose only exception is the allowed Info is **not** reviewed, because there is no trade-off to judge. *Fails:* `cc7_g_a_clean_encode_contributes_no_review_task` — a scenario whose verification raises no Warning produces no `(g)` blind entry, asserted by count.

### 4.1 Budgets — `budget | measured | margin`

Linux, `fallback_gpu()` / llvmpipe, probe-1 unless marked. `cc7_every_budget_carries_the_declared_margin` asserts `budget / measured ≥ 2`.

| Term | Constant | Budget | Measured (Linux) | Margin |
| --- | --- | ---: | ---: | ---: |
| neutral spread, matched cam B | `CC7_MATCH_NEUTRAL_SPREAD_MAX_CODE` | **5** | **2** (P2) | **2.50×** |
| chart luma mean delta | `CC7_MATCH_LUMA_MEAN_MAX_CODE_MILLIONTHS` | **5 000 000** | **−1 381 567** (P2) | **3.62×** |
| (b1) residual spread | `CC7_MATCH_NEUTRAL_SPREAD_MAX_CODE` (reused) | **5** | **2** (P1; re-measured at §12 step 5) | **2.50×** |
| log first percentile (floor), **16-bit** | `CC7_LOG_FIRST_PERCENTILE_MIN_CODE16` | **5 140** | **7 196** carrier / **2 570** cam A (P3) | +2 056 / −2 570 codes |
| log 99th percentile (ceiling), **16-bit** | `CC7_LOG_P99_MAX_CODE16` | **51 400** | **42 919** carrier / **62 194** cam A (P3) | −8 481 / +10 794 codes |
| log inverse patch error (set-wide) | `CC7_LOG_INVERSE_MAX_CODE` | **12** | **4** at size 65 (P3) | **3.00×** |
| feather counts | `CC7_FEATHER_PARTIAL_TOLERANCE_PIXELS` | **4** | **0** against the discrete model (P5) | see note 3 |
| deep-shadow gamut count | `CC7_LOOK_DEEP_SHADOW_OUT_OF_GAMUT_PIXELS` | **192** exact | **192** (P2, amended scene) | exact |
| delivery, 8-bit luma max | `DELIVERY_LUMA_MAX_CODE_8BIT` (CC6) | 8 | **2** (P7) | 4.00× |
| delivery, 8-bit luma P99 | `DELIVERY_LUMA_P99_CODE_8BIT_MILLIONTHS` (CC6) | 3 000 000 | **1 000 000** (P7) | 3.00× |
| delivery, 8-bit luma mean | `DELIVERY_LUMA_MEAN_CODE_8BIT_MILLIONTHS` (CC6) | 400 000 | **377 538** (scenario (e), C-E8; was 185 059 P7) | **1.06×** — recorded, not cleared (§0.3 R4-M2) |
| delivery, 8-bit RGB mean | `DELIVERY_RGB_MEAN_CODE_8BIT_MILLIONTHS` (CC6) | 1 750 000 | **499 781** (P7) | 3.50× |
| delivery, 8-bit PSNR | `DELIVERY_PSNR_FLOOR_DB_HUNDREDTHS_8BIT` (CC6) | ≥ 3 300 | **4 148** (P7) | +8.48 dB |
| delivery, 10-bit luma max | `DELIVERY_LUMA_MAX_CODE_10BIT` (CC6) | 16 | **1** (P7) | 16.00× |
| delivery, 10-bit luma P99 | `DELIVERY_LUMA_P99_CODE_10BIT_MILLIONTHS` (CC6) | 4 000 000 | **0** (P7) | see note 3 |
| delivery, 10-bit luma mean | `DELIVERY_LUMA_MEAN_CODE_10BIT_MILLIONTHS` (CC6) | 1 000 000 | **972** (P7) | 1 029× |
| delivery, 10-bit RGB mean | `DELIVERY_RGB_MEAN_CODE_10BIT_MILLIONTHS` (CC6) | 1 000 000 | **199 455** (P7) | 5.01× |
| delivery, 10-bit PSNR | `DELIVERY_PSNR_FLOOR_DB_HUNDREDTHS_10BIT` (CC6) | ≥ 3 300 | **4 190** (P7) | +8.90 dB |
| track confidence floor | `CC7_TRACK_MIN_CONFIDENCE_BASIS_POINTS` | **8 500** | occluded max **7 411** / clean min **9 740** (P2) | **+1 089 / −1 240 bp** |
| track observation error | `CC7_TRACK_TOLERANCE_BASIS_POINTS` | **200** | **49** bp worst clean raw observation (P2) | **4.08×** |
| (f) containment half-extents | `CC7_TRACK_WINDOW_HALF_{WIDTH,HEIGHT}_BASIS_POINTS` | 563 / 1 000 bp (18 px) | required **14.77 / 12.88** px (P2) | **3.23 / 5.12** px |

Reported, never gated (P7): 8-bit `red 52 / 4 000 000 / 435 573`, `green 19 / 2 000 000 / 269 524`, `blue 62 / 4 000 000 / 794 247`; 10-bit `red 205 / 15 000 000 / 231 532`, `green 74 / 5 000 000 / 75 221`, `blue 242 / 17 000 000 / 291 612`. These are the 4:2:0 hard-edge terms `DELIVERY_RGB_EXTREMES_NOTE` covers, comparable to CC6's own reported 63 / 242.

Four notes follow this table, in CC6 §6.3's shape:

1. **Distinctness.** §2.6's assertion list; no CC7 constant may equal a compositor, delivery, or tracking constant it could be silently substituted for. `CC7_TRACK_MIN_CONFIDENCE_BASIS_POINTS` is explicitly asserted `!= 5_000`, the tracker default, because probe-1 showed the default drops nothing.
2. **No delivery constant is re-baselined.** CC6 owns `DELIVERY_*`; CC7 measures against them and records margins. The worst is the 8-bit luma mean at **1.06×** on scenario (e) (**superseded from 2.16×**, which was probe-1's pre-A1 (a)-only figure — §0.3 C-E8 / R4-M2): it does **not** clear the 2× bar, the delivery rows carry a margin-recording `Cc7BudgetKind` rather than the ≥ 2× assertion, and the pre-authorised fallback is that the (e) 8-bit lane becomes reported-not-gated if Windows CI overruns it.
5. **The track confidence floor is a two-sided bound, not a ratio.** `8 500` sits **+1 089 bp** above the measured occluded maximum and **−1 240 bp** below the measured clean minimum, on a **2 329 bp** separation; the ≥ 2× rule does not apply to a value pinned between two populations, and the manifest records both sides rather than a fabricated margin. The strict "half the gap each side" rule admits the single value 8 576; **8 500** is the round number nearest it that keeps more than 1 000 bp both ways.

3. **Terms measured at or near zero** record `margin_ratio: "infinite (measured exactly zero)"` and name their failing-direction fixture as the bound rather than a fabricated ratio: the 10-bit luma P99 (bounded by the starved encode), `CC7_MATTE_OUTSIDE_CHANGED_PIXELS_MAX = 0`, the (d)(3) hue delta, and the feather counts (measured error 0 against an exact discrete model, where the tolerance absorbs basis-point quantization rather than measurement noise). **The three 10-bit rows with margins above 10× — luma max 16.00×, luma mean 1 029×, and the P99 above — are CC6 constants CC7 must not move** (§4(g)(1)): a slice that measures against another slice's budget records the margin it finds and does not tighten a constant it does not own. Their bound is CC6's own starved-encode fixture, not CC7's.
4. **Sanity floors versus measurements.** `CC7_LOG_FIRST_PERCENTILE_MIN_CODE16` and `CC7_LOG_P99_MAX_CODE16` are *signature* bounds chosen to separate the carrier from cam A, not codec measurements; §4(c)(1)'s failing direction is what bounds them, and they are stated as a code distance rather than a ratio.

### 4.2 Failing directions, same terms

| Term | Failing fixture | Measured in the failing direction |
| --- | --- | ---: |
| neutral spread (1) | `cc7_a_the_unmatched_candidate_exceeds_the_neutral_spread_budget` | **6** (unmatched B — the reason the budget is 5, not 6) |
| neutral spread (2) | `cc7_a_the_unrecoverable_candidate_exceeds_the_neutral_spread_budget` | **19** (corrected C2, 3.8× over) |
| chart luma mean | `cc7_a_the_unmatched_candidate_exceeds_the_luma_mean_budget` | **−19 904 917** (unmatched B, 3.98× over) |
| (b1) clamp | `cc7_b_c1_publishes_no_clamp` | `clamped == false` on all three controls |
| (b2) range excursion | `cc7_b_c1_raises_no_range_excursion` | **0 bp**, no exception, per-node `0 / 0` |
| log signature | `cc7_c_the_base_scene_does_not_read_as_log` | cam A **2 570 / 62 194** (16-bit) — fails both bounds |
| log inverse | `cc7_c_an_identity_cube_does_not_undo_the_log_curve` | identity 33³ cube, set-wide worst **85** (7.1× over) |
| lattice sweep | `cc7_c_the_cube_size_sweep_is_monotone_and_size_seventeen_fails` | **13** at size 17 (over 12) |
| feather counts | `cc7_d_feather_zero_has_no_partial_pixels` | `partial == 0`, `covered == full == 192` |
| qualifier containment | `cc7_d_a_qualifier_that_selects_two_patches_is_rejected` | > 192 covered; the pre-A1 scene measured **320 / 192 / 128** |
| gamut count | `cc7_e_the_base_scene_without_the_look_is_in_gamut` | **0** on the ROI and the raster |
| bypass equality | `cc7_e_after_does_not_match_absent` | before ≠ after hashes |
| track observation | `cc7_f_observation_gate_rejects_a_doubled_offset` | `2 × 200` bp |
| track confidence | `cc7_f_the_default_floor_drops_no_sample` | `low_confidence_samples == []` at 5 000 **and** at 7 000 |
| (f2) typed refusal | `cc7_f2_the_default_floor_does_not_refuse` | the same `0..48` step 47 call at 5 000 **succeeds** |
| containment | `cc7_f_a_window_smaller_than_the_square_loses_containment` | the seeded 1.0× window is **2.77 px** short in x |
| delivery budgets | `cc7_g_a_starved_encode_trips_the_decoded_difference_budget` | per term at `-b:v 100k`, measured at §12 step 5 |


---

## 5. The agent path — scripted end-to-end tests

`crates/kinewright-agent/tests/mcp_server.rs`, one `cc7_`-prefixed `#[tokio::test]` per scenario, following the CC5/CC6 templates (`:594`, `:1613`). Each drives the **real** MCP endpoint over `McpServer::start(core, media, media)` + `StreamableHttpClientTransport` (`:601-603`) with **scripted** tool calls: there is no LLM in this section, and no `AgentDriver`. Shared helpers `invoke_capability` (`:2031`), `prepare_plan` (`:2049`), `commit_request` (`:2070`), `query_document` (`:2087`), `resolve_plan_confirmation` (`:2096`) are reused unchanged.

### 5.1 Uniform assertions

Every one of the six tests asserts, in this order:

1. every planner/inspector response carries `evidence_only: true` and `applied: false`, and the document effect count is unchanged after planning;
2. a stale `expected_revision` on the planner returns the typed `stale_revision { expected_revision, actual_revision }` (`color_scopes.rs:600-609`) and `is_error == true`;
3. `prepare_edit_plan` → `commit_edit_plan` advances `timeline_revision` **exactly once** per commit, asserted by reading the revision before and after;
4. the committed document, re-read through `query_document`, **equals** `cc7_scenarios::cc7_canonical_operations(scenario)` applied to the scenario's initial document — effect names, effect order, every stored parameter, and every keyframe — and the neutral controls the planner did not move are asserted **absent** (`tests/mcp_server.rs:936-950`). **This is a regression pin, and the contract says so (R-M8):** the (a)/(b1)/(b2) `exposure_milli_stops` / `temperature_percent` / `tint_percent` values in §2.5 are exactly what `match_parameters` (`crates/kinewright-agent/src/color_scopes.rs:1860-1965`) produces, so the equality is "the planner still does what it did when this was measured", not an independent derivation. Rule 11.0.1 is satisfied because the numbers came from an **independent `f64` transcription** of `:1860-1965` — the method probe-2 used — and never from calling the tool and writing down its answer. The (f) keyframe values are pinned the same way and for the same reason;
5. the same integers are re-read from `get_color_context`'s `color_nodes` manifest, so the document and the agent-visible manifest cannot disagree (`:900-935`, `:1131-1160`).

### 5.2 The six scripts

| Test | Tool-call script | Typed codes asserted |
| --- | --- | --- |
| `cc7_a_mixed_camera_match_retains_the_reference_and_lands_the_canonical_grade` | `analyze_color_shot`{clip 1} → `analyze_color_shot`{clip 2} → `plan_shot_match`{`reference_clip_id: 1`, `candidate_clip_ids: [2]`, `roi: (0,2000,3000,888)`} → `prepare_edit_plan` → `commit_edit_plan` → `get_color_qc`{`checks:["skin"]`, `roi: (0,4223,1500,888)`, clip 2} → `render_color_proof`{clip 2, `effect_id`, `look_comparison: "after"`} | `stale_revision`; `invalid_request` for reference-as-candidate |
| `cc7_b_wrong_balance_publishes_the_clamp_and_the_range_warning` | `plan_shot_match`{ref 1, candidates `[2]`} on the C1 document, then on the C2 document → prepare/commit (C2) → `get_color_qc`{`checks:["range","gamut","tags","per_node"]`, `max_nodes: 16`} | `stale_revision`; `color_qc_node_budget_exceeded` at `max_nodes: 17` |
| `cc7_c_log_like_input_is_normalised_by_an_imported_technical_lut` | **branch server started with the project-path handle** (copying `cc4_branch_server_with_the_project_path_handle_resolves_imported_availability`, `mcp_server.rs:1351`) → `import_lut_asset`{path} → `list_look_assets` → `plan_technical_lut`{`lut_asset_id`, `input_encoding_token: 0`} → prepare/commit → `get_color_qc` → `render_color_proof` | `project_not_saved` without the handle (`server.rs:352`); `missing_lut_asset` for an unregistered id |
| `cc7_d_product_qualifier_selects_its_patch_and_leaves_skin_alone` | `plan_secondary_correction`{`node_kind: "primary_correction"`, `derive_qualifier_from_sample: true`, `sample_roi: (1500,4223,375,888)`, `saturation_percent: 40`} → prepare/commit → `inspect_grade_matte` → `get_color_qc`{`checks:["skin"]`, `roi: (0,4223,1500,888)`} | `matte_unsupported_node_kind` for `technical_lut`; `color_qc_region_required` with no region |
| `cc7_e_creative_look_bypass_matches_absent_and_reports_its_gamut` | `plan_creative_look`{built-in `warm` asset id, `mix_basis_points: 10_000`} → prepare/commit → `render_color_proof` ×3 (`before`, `after`, `bypass`) → `get_color_qc`{`checks:["gamut"]`} → `list_look_assets` | `look_comparison_requires_effect_id`; `bypass_not_lossless` is asserted **absent** |
| `cc7_f_tracked_secondary_drops_only_the_occluded_samples` | `plan_secondary_correction`{window from §2.3.6} → prepare/commit → `track_matte_window`{`window_index: 0`, `start_local_frame: 0`, `end_local_frame: 48`, `step_frames: 5`, `search_radius_percent: 10`, `max_width: 256`, `minimum_confidence_basis_points: 8500`} → prepare/commit → `inspect_grade_matte` at frames `0, 9, 18, 28, 42` → the (f2) call over `0..48` at `step_frames: 47` | `tracking_confidence_too_low` (f2, all four fields + `recovery_action`); `matte_window_index_out_of_range` at index 4 |

**The shape of `observations[]` and `low_confidence_samples[]` (R-M17).** Both lists hold the **same object shape**, built once per sample at `server.rs:4563-4580` and pushed into one list or the other: `local_frame`, `project_frame`, `center_x_basis_points`, `center_y_basis_points`, `composite_center_x_basis_points`, `composite_center_y_basis_points`, `center_pixel`, `confidence_basis_points`. Every CC7 assertion about "which frames" maps **`.local_frame`**; every assertion about position reads **`center_{x,y}_basis_points`, which is layer space** — the composite value is a separate key and the two must not be compared to one another. Per A17, containment and tolerance gates read `observations[]` and never `curves`, whose final keyframe carries the tool's published `known_systematic_lag` (`server.rs:4701`), measured at **746 bp**.

### 5.3 The GPU-unavailable rule

Every scenario renders through `matte_proof_for_document`, `working_proof_for_document`, `monitor_proof_for_document`, or the compositor. Where a skip branch exists, it follows the CC5/CC6 template exactly (`tests/mcp_server.rs:826-846`, `:1734-1752`):

- a refusal is accepted **only** when `std::env::var("KINEWRIGHT_GPU_TESTS_MAY_SKIP") == Ok("1")`;
- the branch then asserts the **typed** code — `matte_proof_unavailable` for `inspect_grade_matte`, `working_proof_unavailable` for `get_color_qc`, `color_proof_render_failed` for `render_color_proof` — **never both branches**, because "a test that accepts both branches asserts nothing";
- both branches print a `SKIPPED:` line and both assert the timeline revision did not move;
- the environment variable is consulted **only** here. It must not appear in `cc7_fixtures.rs`, `cc7_sources.rs`, or any media/core CC7 file, and §11.3's `uses_outside_prose` guard asserts that.

### 5.4 M36 bookkeeping — the registry and the served surface are UNCHANGED

CC7 adds **no** tool. Concretely and normatively:

- `COMPACT_TOOL_NAMES` stays at **7** (`crates/kinewright-agent/src/runtime.rs:17`); `served_surface_is_small_and_keeps_the_internal_registry_discoverable` (`server.rs:20939`) is asserted to still report `served.tool_count == 7`, `served.tool_count < registry/4`, and `served.serialized_bytes < registry.serialized_bytes / 4`, and CC7 asserts the served byte counts are **byte-identical to CC6's** `5 660 / 3 510 / 998`.
- `INSPECTOR_TOOL_NAMES: [&str; 75]` stays at **75** (`schema.rs:16`), asserted at `server.rs:19094`. No `schema.rs` array entry, no count change, no `CAPABILITY_KIND_OVERRIDES` entry.
- The registry stays at **124 tools / 1 280 060 B**. `docs/M36-AGENT-RUNTIME-EFFICIENCY.md` gains two appended rows recording *the same numbers* with the note "CC7 adds no tool; the surface is unchanged", because M36's rows are appended and never edited.
- The pinning test is `cc7_the_agent_surface_is_unchanged_by_this_slice`, which asserts all four numbers above against the CC6 row.

### 5.5 Typed codes CC7 asserts

**CC7 introduces no new typed code.** Both tables below list codes that exist at `99faee3` and that CC7 gates on. *Every code in this table appears in no other table, and each carries `code`, `field`, `observed`, and `allowed`.*

| Code | Severity | Raised when | Owner |
| --- | --- | --- | --- |
| `delivery_range_excursion` | `Warning` | corrected C2 clips ≥ `QC_RANGE_EXCEPTION_BASIS_POINTS = 10` bp of the raster (§4(b)(3)) | `kinewright-core::color_qc` |
| `delivery_gamut_excursion` | `Warning` | the `warm` look drives blue below zero on ≥ 10 bp (§4(e)(2)) | `kinewright-core::color_qc` |
| `skin_region_outside_band` | `Info` | the failing direction of §4(a)(4), an ROI over `product_red` | `kinewright-core::color_qc` |
| `decoded_range_excursion` | `Warning` | §4(g)(3)'s conditional trigger for the human question | `kinewright-media::verify` |
| `decoded_difference_over_budget` | `Error` | §4(g)(1)'s starved-encode failing direction | `kinewright-media::verify` |
| `qc_per_node_truncated` | `Info` | asserted **absent** at `max_nodes: 16` on every CC7 document | `kinewright-core::color_qc` |

*Every code in this table appears in no other table, and each carries `code`, `field`, `observed`, and `allowed`.*

| Code | Type | Raised when |
| --- | --- | --- |
| `stale_revision` | planner/inspector refusal | §5.1(2), every scenario |
| `tracking_confidence_too_low` | tracking refusal | §4(f)(4), the (f2) range |
| `matte_window_index_out_of_range` | matte refusal | `window_index: 4` on a one-window node |
| `matte_unsupported_node_kind` | matte refusal | `plan_secondary_correction` with `node_kind: "technical_lut"` |
| `color_qc_region_required` | QC refusal | `checks: ["skin"]` with neither `roi` nor `matte_region` |
| `color_qc_node_budget_exceeded` | QC refusal | `max_nodes: 17` |
| `missing_lut_asset` | look refusal | `plan_technical_lut` naming an unregistered asset |
| `project_not_saved` | look refusal | `import_lut_asset` before the project is saved |
| `unsupported_source_transfer` | source refusal | **cited from CC1**, `cc1_fixtures.rs:3139/3146/3153`; not re-asserted |
| `working_proof_unavailable` / `matte_proof_unavailable` | GPU-unavailable refusal | §5.3's skip branch only |

**No consumer parses a `Display` string to recover a CC7 code**, and no CC7 refusal site emits prose of its own; the human sentence is composed from the four fields at the display surface (E19/E32's rules, unchanged).

---

## 6. The person path

D1 pins the shape: the person path is proven **at the operation-builder level**, not by driving a live `KinewrightApp`. There is no app harness — `KinewrightApp::new` is private, takes an `Arc<FfmpegMediaEngine>`, and has one call site (`app.rs:180`, `:1858`); there is no `crates/kinewright-app/tests/` directory. Building one is a slice, not a flag (§13).

Each test below is an inline `#[test]` in the module that owns the builder, is named in the §11.2 inventory, drives the builder to a `Vec<Operation>`, feeds it through core with `apply_batch` **in order**, and asserts the resulting document equals `cc7_scenarios::cc7_canonical_operations(scenario)`'s document. This is the established pattern (`every_curve_edit_batch_is_accepted_by_core_in_order`, `inspector_ui.rs:5001`).

| Scenario | Builder(s) | Test |
| --- | --- | --- |
| (a) | `InspectorEdits` (`app.rs:125-234`) + `primary_correction_section`'s ten integer sliders (`inspector_ui.rs:2103-2199`), writing `exposure_milli_stops`, `temperature_percent`, `tint_percent` through the coalesced path (`primary_coalesce_key`, `:251`) | `cc7_a_a_person_can_author_the_matched_primary_by_hand` |
| (b) | the same three sliders; the descriptor bound `temperature_percent ∈ −100..=100` (`effect.rs:571-585`) **is** the person-visible control limit, and the test asserts the slider's range equals the descriptor's, so the human meets the same clamp the planner published | `cc7_b_the_temperature_slider_stops_where_the_planner_clamps` |
| (c) | `lut_import_operations(document, import, &LutImportIntent::Apply { clip: Some(clip), stage: ColorStage::Input })` (`media_workflow.rs:1634-1670`) → `insert_lut_node_operation` (`inspector_ui.rs:323-341`), which emits `AddLutAsset` then one `InsertEffect` `technical_lut` carrying only `lut_asset_id` at `color_stage_insert_index` | `cc7_c_a_person_can_import_and_bind_the_technical_lut` |
| (d) | `matte_section_body` (`inspector_ui.rs:2681-2866`) rows: `Enable matte`, the nine `MATTE_QUALIFIER_LABELS` integer controls (`:2338-2349`), `Matte mix`, plus the `saturation_percent` slider. The person types the **same nine resolved values** the agent's `derive_qualifier_from_sample` produced (§2.5); the app has no eyedropper and CC7 does not add one | `cc7_d_a_person_can_author_the_product_qualifier_by_hand` |
| (d2) | `matte_window_row` (`inspector_ui.rs:2876-2995`), whose nine controls are shape, centre X, centre Y, half width, half height, **rotation**, feather, **invert**, and the **"Select in viewer" / "Remove"** actions — the (d2) test drives the seven that carry values and asserts the two actions exist | its **own** test, because (d2) is a separate window-only node (R-B4) |
| (e) | `builtin_look_operations(document, clip, BuiltinLook::Warm, ColorStage::Look)` (`look_browser_ui.rs:199-201`), the **Add as new look** path; the look card's mix row (`look_mix_row`, `inspector_ui.rs:1820-1846`) writes `mix_basis_points = percent · 100` and is asserted to leave the neutral `10_000` unstored | `cc7_e_a_person_can_add_the_built_in_warm_look` |
| (f) | **person-N/A by construction** | see below |

**Scenario (f) is person-N/A.** `Track window…` is permanently disabled: `matte_track_button_enabled() -> false` (`inspector_ui.rs:2868-2872`), whose tooltip states the reason verbatim — *"Tracking is agent-driven in CC5: ask the agent to run track_matte_window. The app has no agent-tool call path, so this button would pretend to work."* CC7 records this here **and** as an explicit deferral in §13 with its cost, rather than quietly counting five scenarios as six. The test `cc7_f_the_person_path_is_not_available_and_says_so` asserts `matte_track_button_enabled() == false` and that the tooltip names `track_matte_window`, so the N/A is a checked fact and not a comment.

**Scorecard consequence, stated measurably** (`ROADMAP:534-547`): "workflows completed end to end by both person and agent" scores **5 of 6** for the person path and **6 of 6** for the agent path in CC7. That number is published in `cc7_manifest.json` under `scorecard.person_agent_parity` and is the thing a later slice moves by enabling the button.

---

## 7. The model path — eval suite `color-workflow-v6`

### 7.1 Registration

Four edits in `crates/kinewright-agent/src/bin/kinewright-eval.rs`, all mandatory:

1. `fn color_workflow_suite() -> Vec<EvalDefinition>` returning six definitions, in the shape of `generalization_suite` (`:1610`), with `#[allow(clippy::too_many_lines)]`.
2. A `eval_suite` arm (`:211-224`): `"color-workflow-v6" | "v6" => Ok(("kinewright-color-workflow-v6", color_workflow_suite()))`, and its error string extended.
3. **`is_packaged_benchmark` (`:226-234`, which lists **four** ids — v1 is deliberately excluded) gains `kinewright-color-workflow-v6` — MANDATORY.** Without it the run produces no artifacts, no review package, and **overwrites `docs/EVALS.md`** (`:277-283`).
4. `print_usage` (`:477-482`) enumerates the suites literally and gains `color-workflow-v6`.

### 7.2 Tasks

Task ids are the first whitespace token of `name` (`filter_definitions`, `:236-256`).

| id | `name` | Fixture builder | Scenario |
| --- | --- | --- | --- |
| `c1` | `"c1 Mixed-camera interview match"` | `fixture_cc7_mixed_camera()` | (a) |
| `c2` | `"c2 Wrong white balance and underexposure"` | `fixture_cc7_white_balance()` | (b) |
| `c3` | `"c3 Log-like input normalisation"` | `fixture_cc7_log_like()` | (c) |
| `c4` | `"c4 Product and skin secondary"` | `fixture_cc7_product_and_skin()` | (d) |
| `c5` | `"c5 Creative look with a gamut exception"` | `fixture_cc7_creative_look()` | (e) |
| `c6` | `"c6 Tracked secondary through an occlusion"` | `fixture_cc7_tracked_secondary()` | (f) |

Every builder imports its media from `kinewright_media::cc7_sources` (§3) — the *same* generator the technical gates use — and returns a `PreparedFixture` whose `original_document` is the scenario's initial document and whose `project_path` is `None` **except for `fixture_cc7_log_like()`**, which saves the project into its temp directory, writes `log_like_inverse_cube(65)` beside it as `log-inverse.cube`, and returns that path (R-B2). Without it `import_lut_asset` refuses `project_not_saved` and c3 is unsatisfiable (§7.6).

### 7.3 Prompts

One user turn per task (`max_turns: 1`, as every existing suite). Each names the clips and the intended outcome and **never names a parameter or a value**; the point of the suite is to find out whether the model can choose them.

- **c1** — *"Clips 1 and 2 are the same interview shot from two cameras. Clip 1 is the reference the colourist approved. Match clip 2 to it so the neutral chart in both reads neutral and the two cut together, leave clip 1 exactly as it is, then show me a proof of the corrected clip and confirm the skin in clip 2 still reads as skin."*
- **c2** — *"Clip 2 was shot on the wrong white-balance preset and badly underexposed; clip 1 is a correctly balanced take of the same scene. Recover clip 2 as far as the primary controls allow, tell me plainly where you ran out of authority, and check whether the recovered clip now pushes anything outside the delivery range."*
- **c3** — *"Clip 1 was recorded on a flat, log-like curve and looks washed out. The project is saved, and a matching inverse LUT is on disk beside it as `log-inverse.cube`. Normalise clip 1 back to a correct Rec.709 rendering using that file, then show me the result and a QC report."*
- **c4** — *"On clip 1, make the red product read richer without touching anyone's skin. Show me exactly which pixels you affected, and prove the skin tones are unchanged."*
- **c5** — *"Give clip 1 a warm evening look using one of the built-in looks. Show me before, after, and the look bypassed at the same frame, and tell me whether the look pushes any pixels out of gamut."*
- **c6** — *"On clip 1, isolate the moving red product with a tracked secondary and lift its saturation. Something crosses in front of it partway through — do not invent a position you did not measure, and tell me which samples you could not trust."*

### 7.4 Budgets

`fn color_workflow_budget(max_tool_calls: usize) -> EvalBudgets` **reuses `EvalBudgets`' field set, not `standard_budget`'s values** (minor 12): `standard_budget` is `max_tokens: 30_000` / `max_wall_time: 5 min` (`kinewright-eval.rs:1716-1731`), and CC7's 60 000 / 15 min is a **new** budget with `max_turns: 1`, `max_operations: 8` (24 for `c6`), `max_cost_usd: 2.00`, and:

| Task | `max_tool_calls` | `max_wall_time` | Rationale |
| --- | ---: | ---: | --- |
| c1 | 16 | 15 min | 2 analyses + match + prepare/commit + QC + proof, with margin |
| c2 | 16 | 15 min | match + prepare/commit + a per-node QC (up to 17 renders) |
| c3 | 16 | 15 min | import + list + plan + prepare/commit + QC + proof |
| c4 | 16 | 15 min | plan + prepare/commit + matte inspect + two QC calls |
| c5 | 16 | 15 min | plan + prepare/commit + three proofs + QC + list |
| c6 | **24** | **25 min** | two commits plus five sampled `inspect_grade_matte` calls |

`SessionConfig.max_turns` is set at **`eval.rs:878`** to `budgets.max_tool_calls + 2` (minor 13); `budgets.max_turns` is only *scored* (`eval.rs:2303`, `:2924-2925`) and is never enforced on the session, so the two must not be confused.

### 7.5 Where the colour measurements are computed, and the new `EvalAssertion` variants

**The plumbing is the design (R-B1).** `evaluate_assertion` (`crates/kinewright-agent/src/eval.rs:2494-2499`, called from `:2469`) is private and synchronous and its only inputs are `&EvalDefinition` (`eval.rs:84-92`) and `&EvalOutcome` (`eval.rs:537-548`). `EvalOutcome` carries **no** `Analysis`, no `Core`, no exporter, no `original_document`, no per-call tool log — `SessionMetrics.tool_calls` is a `BTreeMap<String, u32>` of aggregate counts (`eval.rs:506`) — and `PreparedFixture`'s `analysis` / `exporter` / `core` (`eval.rs:457-465`) are consumed inside `run_eval_with_artifacts` (`eval.rs:856-960`) and dropped before `evaluate` runs (`eval.rs:933-950`). A colour assertion written as "an arm in `evaluate_assertion`" is therefore **unreachable**. CC7 does it the other way round:

1. **Measure inside `run_eval_with_artifacts`**, after the session and after the deliverable step, where `fixture.analysis`, `fixture.exporter`, `core` and the original document are all still alive (`eval.rs:864-870`).
2. **Carry the result on `EvalOutcome`** as one typed block, plus the original document:

```rust
pub struct EvalOutcome {
    // ... unchanged fields ...
    pub original_document: Document,
    pub color: Option<ColorEvalEvidence>,
}

pub struct ColorEvalEvidence {
    pub neutral_spread_max_code: Option<i64>,          // over CC7_CHART_PATCHES, monitor proof
    pub chart_luma_mean_delta_millionths: Option<i64>,
    pub skin: Option<SkinDiagnostics>,                 // kinewright_core::color_qc
    pub matte: Option<MatteCoverageStatistics>,        // kinewright_core::media, ROI-cropped
    pub gamut_pixel_count: Option<u64>,                // deep_shadow ROI
    pub qc: Option<ColorQcReport>,
    pub verification: Option<DeliveryVerification>,
    pub look_bypass_matches_absent: Option<bool>,
    pub final_effects: Vec<EffectSummary>,             // clip, effect, name, parameters, keyframes
}
```

3. **The eight variants read only that block and the two documents.** None needs a tool log, which is why `TrackLowConfidenceSamplesExactly` is **replaced**.

| Variant | Fields | Reads |
| --- | --- | --- |
| `ColorQcTechnicalPass` | `clip_id: u64, frame: i64, checks: Vec<String>` | `outcome.color.qc.technical_pass` |
| `DeliveryVerificationWithinBudgets` | `depth: DeliveryEncodeDepth` | `outcome.color.verification.{comparison.within_budgets, technical_pass, tags}` |
| `NeutralPatchSpreadAtMost` | `patch_rois: Vec<NormalizedRoi>, maximum_code: i64` | `outcome.color.neutral_spread_max_code` |
| `ReferenceClipUntouched` | `clip_id: u64` | `outcome.original_document` against the final document |
| `SkinHueWithinBand` | `roi: NormalizedRoi, minimum_in_band_basis_points: u32` | `outcome.color.skin.in_band_basis_points` |
| `MatteContainmentExact` | `roi: NormalizedRoi, expected_covered_pixel_count: u64, expected_full_pixel_count: u64, expected_partial_pixel_count: u64` | `outcome.color.matte` |
| **`TrackKeyframesMatchExpected`** | `parameter: String, expected_local_frames: Vec<i64>, absent_local_frames: Vec<i64>` | the **committed document's keyframes** on `matte_window0_center_{x,y}_basis_points` — present at the ten surviving samples, **absent at 47** |
| `LookBypassMatchesAbsent` | `clip_id: u64, effect_id: u64, frame: i64` | `outcome.color.look_bypass_matches_absent` |

`TrackKeyframesMatchExpected` exists because there is no per-call tool log anywhere in `SessionMetrics` and CC7 does not add one; the committed keyframes are the durable evidence of what the tracker did, and they are already the thing §4(f) gates in the `cargo test` lane.

**Thresholds are variant fields, and the suite call site reads a constant (R-M18).** Every existing variant already carries its threshold as a field (`eval.rs:167-435`: `tolerance_frames`, `minimum_aligned_basis_points`, `maximum_step_percent`, …); what varies is whether the *call site* writes a literal. CC7's rule is that `color_workflow_suite()` passes a `cc7_scenarios` constant into every threshold field, never a literal — so a re-baseline moves one constant and not six suite lines.

### 7.6 `EvalDeliverableSpec.delivery_bit_depth`, `PreparedFixture.project_path`, and `EvalMeasurement`

**`EvalDeliverableSpec` has no serde derives** — `#[derive(Debug, Clone, Copy, PartialEq, Eq)]`, `eval.rs:94-107` — so there is no `#[serde(default)]` to add and no JSON to migrate (R-B7). The field is a plain one:

```rust
pub struct EvalDeliverableSpec {
    // ... unchanged fields ...
    pub delivery_bit_depth: DeliveryEncodeDepth,
}
```

and **every existing `EvalDeliverableSpec { … }` struct literal gains `delivery_bit_depth: DeliveryEncodeDepth::Eight`** — a compile-time change at roughly five call sites in `crates/kinewright-agent/src/bin/kinewright-eval.rs` (the v2/v3/v4/v5 suite constructors, e.g. `:1679-1691`), named in §12 step 8. **No `Default` impl is added and no `..Default::default()` is used**, so a future field cannot be forgotten silently.

**Three hard-coded `DeliveryEncodeDepth::Eight` sites, not two (R-M13):** `eval.rs:992` in `produce_deliverable` and `eval.rs:1132` in `export_and_probe` both have the spec in scope — `export_and_probe(result, spec: EvalDeliverableSpec, …)` (`eval.rs:1115-1121`) already takes it by value — and both read `spec.delivery_bit_depth`. The third, `eval.rs:6157` inside `evaluate_caption_safe_area`, has **no spec in scope and is explicitly out of scope**: captions are not a colour deliverable and CC7 does not plumb a depth through the caption path.

**`PreparedFixture.project_path` (R-B2).** `run_eval_with_artifacts` starts the server with `McpServer::start(core, playback, analysis)` (`eval.rs:866-870`), which reaches `start_with_broker` with a fresh `Arc::new(RwLock::new(None))` project path (`server.rs:297-302`, `:433-457`), so `import_lut_asset` refuses `project_not_saved` for **every** eval run today (`server.rs:1878-1888`). CC7 adds

```rust
pub struct PreparedFixture {
    // ... unchanged fields ...
    pub project_path: Option<PathBuf>,
}
```

and the runner calls `server.set_project_path(Some(path))` (`server.rs:524`) after `McpServer::start` when it is `Some`. **This is a change to the shared eval runner and affects all six suites**; v1–v5 pass `None` and are byte-unchanged. `fixture_cc7_log_like()` saves the project into its temp directory and writes the inverse `.cube` beside it, and c3's prompt names the file.

**`EvalMeasurement`.**

```rust
pub struct EvalMeasurement { pub name: String, pub observed: i64, pub budget: i64, pub unit: String, pub passed: bool }
```

on `EvalResult` as

```rust
#[serde(skip_serializing_if = "Vec::is_empty")]
pub measurements: Vec<EvalMeasurement>,
```

**and nothing else (R-M3).** `EvalResult` derives `Serialize` only (`eval.rs:557`) and nothing ever deserializes it — `results.jsonl` is write-only (`render_jsonl`, `eval.rs:6516-6532`) — so `#[serde(default)]` would be inert and a "pre-CC7 record still parses" claim would be meaningless. What is asserted instead is that a result **without** measurements serialises **byte-identically** to today (§11.2.32). `AssertionResult` and `render_scoreboard` (`eval.rs:6449`) are **unchanged**: measurements add no column. They are consumed by `machine-report.json` and by the manifest test.

Each of the eight variants emits **both** an `AssertionResult` (bool, unchanged) and an `EvalMeasurement`, so the number reaches `results.jsonl` as data rather than as free text inside `detail`.

Each of `c1..c6` carries a spec at `Eight`; the **Ten** lane for every scenario is exercised by §4(g)'s `cargo test` fixtures rather than by six extra model sessions (A10), so the suite's cost stays at one deliverable per task while the delivery evidence stays complete at both depths.
### 7.7 Published benchmark

`benchmarks/auto-edit/v6/` with three files, keys matching the existing manifest tests (`published_v5_...`, `:4271`):

- **`manifest.json`** — `schema_version`, `benchmark_id: "kinewright-color-workflow-v6"`, `title`, `runner`, `implementation`, `fixture_provenance` (naming `kinewright_media::cc7_sources` and the FFV1 recipe), `score_layers` (`technical_gates`, `model_workflow`, `blind_human_review`), `acceptance_target`, and `tasks[]` with per-task `id, name, fixture, ground_truth, prompt, delivery, budget, machine_assertions`.
- **`ground-truth.json`** — the canonical document per scenario (effects, parameters, keyframes) and the per-scenario expected measurements, **generated by a test-only writer `cc7_scenarios::ground_truth_json()`** (Q4). `published_v6_manifest_tracks_the_color_workflow_suite` asserts the **checked-in bytes equal the generated bytes**. Regeneration is deliberately **not** an environment knob: there is no `KINEWRIGHT_UPDATE_GROUND_TRUTH`, because §10 forbids env-conditional behaviour in a gate. When the two diverge the test **prints the generated bytes and the diff** and the implementer pastes them into the file, so a ground-truth change is always a reviewed edit.
- **`README.md`** — what the suite proves, how to run it, the fixture provenance, and the note that the technical gates are `cargo test` and do not need this suite.

**`published_v6_manifest_tracks_the_color_workflow_suite`** asserts, in both directions: the manifest's task ids equal the suite's task ids; each task's `machine_assertions` count equals the definition's assertion count; each `budget` equals the definition's `EvalBudgets`; each `prompt` equals the definition's single prompt string; each `delivery.delivery_bit_depth` equals the spec's; and the ground-truth document for each scenario equals `cc7_canonical_operations` applied to that scenario's initial document.

### 7.8 CI and the real-harness run

CI runs **no** eval (`.github/workflows/ci.yml` is fmt/build/test/clippy only). What CI covers is the suite's *unit* tests: construction, assertion dispatch against synthetic inputs, packaging, and `published_v6_...`. `FakeDriver` gains nothing new. **Running v6 against a real subscription harness is Riel's action, exactly as M40's was**; `docs/EVALS.md` records the run line, the suite table row, and a `## Baseline snapshot` entry reading "pending real-harness run" until it happens.

---

## 8. The blind review package

### 8.0 What "blind" means here, and what it does not

**Definition (R-B8), normative.** A blind reviewer sees **the artefact and the scenario's question, and nothing that says how the machine judged it or which run, sample, harness or model produced it.**

**What is deliberately NOT blinded: the scenario identity.** It is inherent in the question — "Does the match preserve natural and intentional differences?" *is* the mixed-camera scenario — and a chart raster is recognisable anyway. CC7 states this plainly rather than claiming a blindness the tooling cannot deliver. What the package does deliver is that the reviewer cannot see the task id, the sample index, the run id, the benchmark id, the harness or model, any machine verdict, any assertion name, or any parameter the agent chose.

### 8.1 `human-review.json` schema_version 2 (the unblinded file)

```rust
pub struct HumanReviewFile {
    pub schema_version: u32,          // 1 or 2 accepted; 2 is written
    pub benchmark_id: String,
    pub run_id: String,
    pub reviewer: Option<String>,
    pub tasks: Vec<HumanTaskReview>,
}

pub struct HumanTaskReview {
    pub task_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blind_id: Option<String>,     // 12 lowercase hex, DERIVED
    pub artifact_sha256: Option<String>,
    pub accepted: Option<bool>,
    pub ratings: HumanRatings,
    pub not_applicable: Vec<HumanRatingDimension>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub questions: Vec<HumanQuestion>,
    pub notes: Option<String>,
}

pub struct HumanQuestion {
    pub id: String,                   // "a", "b", "d", "e", "f", "g"
    pub prompt: String,               // the matrix's question, verbatim
    pub answer: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
}
```

**This file is not what the reviewer opens.** It names the task, the run and the benchmark, so §8.2's `blind/review-form.json` is the reviewer's copy and this one is either the pre-filled template (for a sighted run) or the file `--score-review` **writes** after unblinding.

**`blind_id` is derived, never random**: the first **12** lowercase hex characters of the task's `artifact_sha256`. Two identical artefacts therefore share a `blind_id`, which makes mechanical the existing "one viewing may be applied to several rows when the artifact hashes are identical" convention (`docs/EVALS.md:82-84`). `blind_id` is `None` for a task with no artefact.

**Backward compatibility.** `schema_version: 1` files still load: the version check at `eval.rs:1698-1703` accepts `1 | 2`, `blind_id` and `questions` default to `None` / `[]`, and a v1 file validates exactly as it does today. `HumanRatingDimension` is **unchanged** — no colour dimension is added (§13).

### 8.2 The `blind/` directory and the key (R-B8)

`write_review_package` (`kinewright-eval.rs:290-322`) writes, for **every packaged run** — no flag, and `print_usage` is unchanged (Q3):

```text
blind/<blind_id>.png            the proof still
blind/<blind_id>.mp4            the finished encode, when the task has one
blind/review-form.json          the ONLY file the reviewer opens
blind-key.json                  in the RUN ROOT, outside blind/
human-review.json               in the run root; written or updated by --score-review
```

`blind/review-form.json` is keyed on `blind_id` **and carries nothing else that identifies anything**:

```json
{ "schema_version": 1,
  "entries": [ { "blind_id": "0f3a1c2d4e5b",
                 "questions": [ { "id": "a", "prompt": "Does the match preserve natural and intentional differences?",
                                  "answer": null, "notes": null } ],
                 "ratings": { "visual_finish": null, "delivery_readiness": null },
                 "not_applicable": ["story", "pacing", "audio_finish", "captions"],
                 "accepted": null, "notes": null } ] }
```

**No `task_id`, no `run_id`, no `benchmark_id`, no harness or model name, no machine result, no assertion name, no parameter name and no parameter value appears anywhere in it.** `blind/` contains those files and nothing else: no README, no subdirectory, no sample index. The `.png` / `.mp4` are byte copies of `artifacts/<task>-sample-<n>/`, so the originals are untouched and the reviewer can be handed `blind/` alone and genuinely see nothing more.

`blind-key.json`, in the run root:

```json
{ "schema_version": 1, "benchmark_id": "...", "run_id": "...",
  "entries": [ { "blind_id": "0f3a1c2d4e5b", "task_id": "c1-sample-1", "sample": 1,
                 "artifact_sha256": "…64 hex…",
                 "artifact_path": "artifacts/c1-sample-1/finished.mp4" } ] }
```

**`--score-review PATH` accepts either file.** Given a v1/v2 `human-review.json` it behaves exactly as today. Given a `blind/review-form.json` it reads the sibling `blind-key.json`, resolves every `blind_id` to its `task_id` and sample, **and does so BEFORE calling `verify_review_artifact_bindings`** — a named change to that function's *caller* (`score_review_file`, `kinewright-eval.rs:637-676`), because `verify_review_artifact_bindings` (`:678-743`) derives ids from `machine_report["results"][].name` (`:694-717`) and looks `task.task_id` up at `:719`; a form keyed on `blind_id` fails there for every task unless resolution happens first. After resolution it writes the unblinded `human-review.json` into the run root and validates as before: `artifact_sha256` must equal the machine-reported `deliverable.output_sha256`, and a rated-but-unbound task is an error. A `blind_id` absent from the key is a typed error naming the id.

### 8.3 The template

`human_review_template` (`eval.rs:1642-1687`) gains, for the colour benchmark id only:

- `blind_id` filled from `artifact_sha256`;
- `questions` populated from the scenario's `Cc7ScenarioSpec.human_question` (§2.2), one entry, `answer: None`;
- `not_applicable` pre-marked with **`Story`, `Pacing`, `AudioFinish`, `Captions`** — the colour suite renders no dialogue, no music and no captions, so rating them would be fabrication.

Consequently `accepted` requires only **`visual_finish`** and **`delivery_readiness`** rated, plus **every question answered**. `summarize_human_review` (`eval.rs:1697`) is extended by exactly one rule: when `accepted` is `Some(_)`, every `questions[].answer` must be `Some(_)`; otherwise it is an error naming the unanswered question id. The 1..=5-in-0.5-increments scale, the rated-and-`not_applicable` conflict rule, the 64-hex `artifact_sha256` rule and the pending-stays-pending rule are unchanged.

### 8.4 What enters the package

One entry per *artefact*, per M40's "at least two outputs per family" shape:

| Scenario | Human question | Enters the package |
| --- | --- | --- |
| (a) | "Does the match preserve natural and intentional differences?" | always |
| (b2) | "Is the proposed compromise acceptable?" | always |
| (c) | — | **never** (objective only; the matrix has no row) |
| (d) | "Does attention remain on the intended subject?" | always |
| (e) | "Does the look support the story?" | always |
| (f) | "Are any visible corrections distracting?" | always |
| (g) | "Only if a codec limitation creates a visible trade-off" | only under §4(g)(3)'s condition |

### 8.5 The leak test

**`cc7_the_blind_package_discloses_no_machine_provenance`** builds a package from synthetic results and scans **both** the `blind/` listing **and the serialised bytes of `blind/review-form.json`** (R-B8), asserting that neither contains:

- any task id (`c1`..`c6`, `-sample-`), the run id, or the benchmark id;
- the strings `agent`, `person`, `model`, `harness`, `passed`, `assert`;
- **any `cc7_scenarios` parameter name** (`exposure_milli_stops`, `temperature_percent`, `tint_percent`, `saturation_percent`, every `matte_*`, `lut_asset_id`, `mix_basis_points`, `input_encoding_token`) or **any canonical parameter value** from §2.5 rendered as a decimal string.

It also asserts the listing length equals the artefact count plus one, that every media name matches `^[0-9a-f]{12}\.(png|mp4)$`, and that `blind-key.json` is **not** inside `blind/`. *Failing direction:* a deliberately leaked copy named `c1-sample-1.mp4` placed in `blind/`, **and** a form entry carrying `"task_id": "c1"`, each asserted to trip the check — so the test is known to be able to fail on both surfaces it scans.

The test does **not** assert the absence of the scenario question or of anything derivable from it; §8.0 says why.

### 8.6 The human gate, M40's wording

CC7's blind review is satisfied when, for each family (scenario) in the package:

> each passes **3/3** model samples against its technical gates; at least **2 of 3** outputs are accepted by a person; **every question is answered**; and the mean human rating is at least **4.0/5** over the applicable dimensions, N/A dimensions excluded. **One success does not satisfy the slice.**

### 8.7 Scorecard outcomes, stated measurably

- **Objective eval coverage versus subjective review time** — per scenario, `cc7_manifest.json` records `objective_assertion_count` (assertions discharged with no human in the loop) against `human_question_count` (0 for (c), 1 otherwise, plus (g)'s conditional). The target is that the human is asked **exactly** the matrix's question and nothing else.
- **Workflows completed end to end by both person and agent** — 6/6 agent, 5/6 person (§6), published as `scorecard.person_agent_parity`.
- **Generalization across footage, edit type, platform, and delivery target** — six scenarios × the six named axes (lighting, skin tones, camera encodings, saturation, motion, deliberate creative exceptions) × two delivery lanes × two CI operating systems, published as `scorecard.generalization_matrix` with a covered/uncovered flag per cell.

## 9. Serialization and migration

1. **Pre-CC7 documents load unchanged.** CC7 adds **no** `Document`, `Effect`, `LutAsset`, or `ExportSettings` field. `ColorPipelineState` stays `managed_sdr_v1`. No effect name, parameter name, unit, or default changes.
2. **`EvalResult.measurements` is `skip_serializing_if` only (R-M3).** `EvalResult` derives `Serialize` and nothing else (`eval.rs:557`); nothing in the workspace deserializes it and `results.jsonl` is write-only (`render_jsonl`, `eval.rs:6516-6532`). There is therefore **no** parse-compatibility claim to make and no `#[serde(default)]` to add — it would be inert. What is guaranteed, and asserted by §11.2.32, is that a result with no measurements **serialises byte-identically to today**, so every checked-in `benchmarks/auto-edit/v{1..5}/*baseline*.json` and every historic JSONL line keeps its exact shape.
3. **`EvalDeliverableSpec` is a compile-time change, not a migration (R-B7).** The struct has **no serde derives** (`eval.rs:94-107`), so `delivery_bit_depth: DeliveryEncodeDepth` is a plain field and **every existing struct literal is edited** to pass `DeliveryEncodeDepth::Eight` — roughly five sites in `kinewright-eval.rs`, named in §12 step 8. No `Default` impl, no `..Default::default()`: a future field must be a deliberate edit at every site rather than a silent zero.
4. **`human-review.json` v1 → v2.** The version check accepts `1 | 2`. `blind_id` and `questions` are `#[serde(default)]` **and** `skip_serializing_if`, so a v1 file round-trips byte-identically through the v2 code and a v2 file with neither field is indistinguishable from v1. `HumanRatingDimension`'s six variants and their wire strings are untouched.
5. **`blind/review-form.json` and `blind-key.json` are new**, both `schema_version: 1`. Neither exists for a pre-CC7 run; `--score-review` on an old run finds no form and no key and takes its existing `human-review.json` path unchanged. A `blind/review-form.json` presented **without** a sibling `blind-key.json` is a typed error naming the missing path, never a silent fallback.
6. **`EvalOutcome` and `PreparedFixture` are internal (R-B1, R-B2).** Neither is serialized anywhere, so `original_document`, `color: Option<ColorEvalEvidence>` and `project_path: Option<PathBuf>` are ordinary field additions. `PreparedFixture` is constructed only inside the fixture builders, so v1–v5 pass `project_path: None` explicitly and the shared runner's `set_project_path` call is a no-op for them.
7. **`ExportSettings`, `ExportJobRecord`, `DeliveryConformanceReport` are unchanged.** CC7 reads them; it adds no field and changes no default.
8. **`cc7_manifest.json` is `manifest_version: 1`** and is a test fixture, not a persisted user artefact; nothing migrates it.
9. **A pre-CC7 `machine-report.json` still scores.** `--score-review` against an old run finds no key, no `blind_id` and no `questions`, and behaves exactly as it does at `99faee3`.

---
## 10. Ordering and determinism

1. **Every gated number is an integer.** Counts are `u64` pixels or samples; rates are integer-floor basis points `floor(value · 10_000 / count)`, `0` for an empty population; fractional terms are `_MILLIONTHS` (`round(v · 1_000_000)`, half away from zero); angles are `_CENTIDEGREES`; dB is `_HUNDREDTHS`; exposure is milli-stops. **No CC7 API returns an `f32` or `f64` to an agent or to the UI**, and no CC7 constant is a float.
2. **Closed-form sampling, no clock and no adaptive stride.** Delivery uses CC6 §6.2's `sample_frames` (`delivery.rs:1319-1338`) unchanged. Tracking uses §2.3.6's transcription of `tracking_sample_frames`. Patch measurement uses the fixed rects of §2.3.3. Nothing samples "until converged".
3. **No RNG anywhere.** `blind_id` is a hash prefix, not a random token; the blind ordering is the artefacts' lexical `blind_id` order, which is deterministic and carries no provenance.
4. **No `Display` parsing.** Typed codes are recovered through the error variant or `recovery_code()`; `MediaError::ColorQc(ColorQcError)` carries QC refusals structurally across the crate boundary (E32).
5. **Iteration orders.** Scenarios iterate in `CC7_SCENARIOS` order; patches in index order (chart `0..=11`, row `0..=6`); channels always `[red, green, blue]`; per-node contributions in track → clip → effect-chain order; exceptions by `(severity desc, code asc, tiebreak asc)`; blind entries by `blind_id` ascending; eval tasks by `c1..c6`.
6. **Percentiles and medians** use the existing lower-median / nearest-rank convention (**`scopes.rs:1319-1338`** — minor 2; `:552-592` is the type declaration, not the convention): `p99` is element `min(n − 1, ceil(0.99·n) − 1)`, median is element `floor((n − 1)/2)`. CC7 does not introduce a second convention.
7. **Pixel iteration is row-major from the top-left**, matching `for_each_linear_pixel` (`compositor.rs:1555-1629`), so a partial-sum reordering cannot change an accumulation. Sums accumulate in `f64`; the fact is recorded in the manifest's provenance.
8. **The generators are pure.** `cc7_base_scene_rgb`, `cc7_camera_scene_rgb`, `cc7_log_scene_rgb`, and `cc7_tracked_scene_rgb` are functions of `(x, y[, frame])` only: no clock, no RNG, no global state. Two runs produce byte-identical `.yuv` input to the muxer, and FFV1 makes the `.mkv` byte-identical on one OS. **Encoded H.264 output is not asserted bit-identical across platforms** (§13).
9. **`cc7_scenarios` has no I/O.** Two evaluations of any of its functions produce identical values on both operating systems, and the manifest asserts the module's constants equal the manifest's numbers rather than the reverse.

---

## 11. Exit fixtures and numeric gates

`crates/kinewright-media/src/cc7_fixtures.rs` (`mod cc7_fixtures;` in `media/src/lib.rs`, `cfg(test)`), `crates/kinewright-media/tests/fixtures/cc7_manifest.json`, `crates/kinewright-core/tests/cc7_core.rs`, agent cases in `crates/kinewright-agent/tests/mcp_server.rs`, eval unit tests in `crates/kinewright-agent/src/eval.rs` and `src/bin/kinewright-eval.rs`, and inline app cases. Every fixture records git revision, backend, adapter, software-fallback and GPU-claim flags, OS, lane, and thresholds through `cc1_fixtures.rs`'s existing `emit_evidence` (`:322-374`).

### 11.0 Fixture-quality rules

**CC6 §11.0.1–9 are carried forward verbatim and unchanged** (`docs/CC6-QC-AND-MANAGED-DELIVERY.md:1352-1362`; `### 11.0` opens at `:1352` and the nine numbered rules run to `:1362`). The manifest's `fixture_quality_rules` key records that range and the inventory test asserts the citation resolves. They are not restated here so that there is one copy; the manifest's `fixture_quality_rules` key cites them by section and the inventory test asserts the citation resolves. In CC7's own terms the four that bite hardest are:

- **11.0.1** — expected values are written analytically in `cc7_scenarios` (§2) or transcribed independently in `f64` in the fixture. **No CC7 fixture may obtain an *expected value* by calling `measure_color_qc` (`kinewright-core::color_qc`), `match_parameters` (`kinewright-agent::color_scopes`), `bt709_limited_ycbcr`, `encode_bt709` / `decode_display709` / `grade709_decode` (`kinewright-media::color_pipeline`), `matte_coverage_statistics` (`kinewright-core::media:724-736`), the compositor, or swscale.** **The source-content exemption is explicit** (minor 8), as CC6 §11.0.1 states it for the BT.709 matrix: `cc7_sources` **must** call the real `grade709_decode` and `encode_bt709` — or an independent transcription of them — to *author* the raster, because authoring content is not asserting an expectation. Nothing in `cc7_sources` is ever compared against the output of a function it called to build the picture, and §11.2.12b is the fixture that keeps `cc7_scenarios`' own transcriptions honest against the pipeline's.
- **11.0.5** — a check that cannot fail is a defect: every gate in §4 names its failing-direction fixture, and §4.2 tabulates them.
- **11.0.6** — GPU fixtures run `fallback_gpu()` in the default lane and `hardware_gpu()` in an `#[ignore]` lane; `KINEWRIGHT_GPU_TESTS_MAY_SKIP` is never consulted by them, and where the agent tests have a skip branch it asserts the typed code (§5.3).
- **11.0.3** — manifest thresholds are asserted **equal to the code constants**, never restated as literals.

### 11.1 Rasters

Three rasters, all §2.3's geometry, all authored by §3:

1. **The base scene**, `320 × 180`, 60 frames, static content, rendered per camera (A, B, C1, C2) and as the log-like carrier. Populations: ramp **6 400**, achromatic chart **1 536**, primaries band **640**, patch row **1 344**, surround **47 680**, total **57 600** (`cc7_base_scene_populations_are_the_contract_table`).
2. **The tracked scene**, `320 × 180`, 100 frames: surround plus the four static skin patches at `y 4..20, x 0..48` plus the moving 24 × 24 `product_red` square at amplitude `(100, 40)` px, occluded on frames **43..=47** (§2.3.6). The seeded window is `375 / 667` bp; the containment gate's 1.5× window is `563 / 1 000` bp.
3. **The `.cube` lattice**, `65³` entries of `clamp(encode_bt709(2^(12e − 8)), 0, 1)` (§3.4), ≈ 8 MB. Written to a `TempDirectory` under the saved project's `<stem>.kinewright-assets` store; nothing is checked in.

The primaries band (A1) is the raster's only hard chroma adjacency, and it is deliberately **four** edges rather than a field of them: CC6 measured RGB max deviation of **63 codes** at one hard blue|green 4:2:0 edge, which is why §4(g) gates only what CC6 gates (luma, RGB mean, PSNR) and reports RGB max and P99 without a bound — probe-1 measured 52 / 62 (8-bit) and 205 / 242 (10-bit) on the CC7 canonical document, comparable to CC6's own reported numbers.

### 11.2 Required fixtures

Every entry names its **manifest key** and its **failing direction**. Every test name below is the name the tree declares; `cc7_declared_test_names_exist_in_their_source_files` (item 30) fails the build if any drifts.

**Core — `crates/kinewright-core/tests/cc7_core.rs` (`CC7_CORE_TESTS`)**

1. **`cc7_scenario_geometry_round_trips_through_normalized_roi`** — key `raster.regions`. Every rect in §2.3.3 and §2.5, `to_pixels(320, 180)`, recovers its pixel rect exactly, including the primaries band's `y 56..72 → (0, 3112, 1250, 888)`; the populations total 57 600. *Fails:* a rect built with `floor` on the start instead of `ceil` misses by one pixel column and is asserted to.
2. **`cc7_chart_and_primary_codes_are_the_contract_table`** — key `patches.chart`, `patches.primaries`. The twelve achromatic codes `[0, 11, 24, 48, 72, 104, 128, 152, 180, 208, 242, 255]` with `R == G == B` on every one, and the five primaries with **no** `[255,0,0]` (A1). *Fails:* a thirteenth entry or a non-achromatic chart entry trips the assertion.
3. **`cc7_camera_a_patch_codes_are_the_hand_derived_display_encoding`** — key `patches.cam_a`. §2.4.1's table, `f64`, within `SPEC_F64_TOLERANCE`, transcribed independently; and the agreement `round(255·encode_bt709(grade709_decode(g))) == round(255·g)` for all seven row patches. *Fails:* a transcription that swaps `decode_bt709` for `grade709_decode` differs on **`skin_light`** — chosen because its grade709 `0.85` is in the **power** segment. `deep_shadow` is deliberately *not* the failing patch: its `0.05` sits below `GRADE709_BETA_ENCODED = 0.081_242_86` (`color_pipeline.rs:921`) and below `decode_display709`'s `0.081`, so all three decodes return `0.05/4.5 = 0.011 111` and agree exactly — a failing direction placed there would pass (R-M14).
4. **`cc7_log_curve_anchors_and_patch_codes_are_the_contract_table`** — key `log.curve`. Also asserts the **unit**: `ChannelStatistics::{first_percentile, ninety_ninth_percentile}` are 16-bit (`= 8-bit × 257`) while `mean_code_values.luma` is an 8-bit mean, so `CC7_LOG_*_CODE16` and the prose equivalents cannot be transposed. `v(1.0) = 2/3` → 170, `v(0.18) = 0.460 506` → 117, and §2.4.2's twelve stored log codes plus the seven row patches. *Fails:* the base scene's own codes differ on at least eight chart patches.
5. **`cc7_log_inverse_error_floors_are_properties_of_the_curve`** — key `log.round_trip`. §2.4.2's exact-inverse error column; the clamped black channel measures **+4** at every lattice size, and the unclamped chart channels measure ≤ 2. *Fails:* a curve without the clamp round-trips black to 0, proving the +4 is the clamp and not the arithmetic (A2).
6. **`cc7_camera_transforms_are_applied_in_linear_light`** — key `patches.cameras`. §2.4.3's measured codes against an independent `f64` transcription; the saturation leg preserves BT.709 luma to `1e-9` on the achromatic patches. *Fails:* the same transform applied in display code space differs on every non-neutral patch and is asserted to.
7. **`cc7_analytic_square_path_stays_in_frame_and_clears_the_patch_row`** — key `tracking.path`. §2.3.6's four generator bounds (`0 ≤ x`, `x + 24 ≤ 320`, `y ≥ 24`, `y + 24 ≤ 180`) over all 100 frames at amplitude `(100, 40)`. *Fails:* an amplitude of 130 px leaves the raster and is asserted to.
8. **`cc7_tracking_sample_frames_are_the_closed_form_distribution`** — key `tracking.sample_frames`. The transcribed `tracking_sample_frames` formula reproduces the tool's list `[0, 4, 9, 14, 18, 23, 28, 32, 37, 42, 47]` for `0..48` step 5, and `[0, 47]` for `0..48` step 47; the occluded subset is `CC7_TRACK_EXPECTED_LOW_CONFIDENCE_FRAMES = [47]`. *Fails:* a naive `start + k·step` stepping gives `0, 5, 10, …` and is asserted **not** to equal it, which is the recipe error A12 corrects made checkable.
9. **`cc7_budgets_are_distinct_from_every_neighbouring_constant`** — key `thresholds.distinctness`. §2.6's list, including `CC7_TRACK_MIN_CONFIDENCE_BASIS_POINTS != 5_000`. *Fails:* a deliberately equal pair trips the check.
10. **`cc7_every_budget_carries_the_declared_margin`** — key `budgets`. Every `budget/measured ≥ 2` and every `measured` strictly inside its budget, over §4.1's table. *Fails:* a measurement equal to its budget trips it.
11. **`cc7_canonical_operations_are_accepted_by_core_in_order`** — key `canonical_documents`. Each scenario's batch through `apply_batch` on its initial document; the result is byte-identical JSON to the manifest's recorded document. *Fails:* a reordered batch is rejected with the typed core error.

**Media — `cc7_fixtures.rs` and `cc7_sources.rs` (`CC7_MEDIA_TESTS`)**

12. **`cc7_base_scene_populations_are_the_contract_table`** — key `raster.populations`. §11.1(1). *Fails:* a one-pixel shift of the primaries band, or a band narrowed or widened by one column, moves an **authored code** at a named edge and is asserted to — the surround *count* is invariant under a shift and cannot carry the claim (§0.3 Z-E2; was "changes the surround count").
12b. **`cc7_core_transcriptions_agree_with_the_pipeline`** — key `transcriptions`. `cc7_scenarios`' own `f64` `encode_bt709`, `decode_display709` and `grade709_decode` (R-M2) agree with `kinewright_media::color_pipeline`'s real functions within `1e-6` at every value in §2.4.1's and §2.4.2's tables, at the twelve chart codes, at the five primaries and across a dense sweep of `−2.0 ..= 2.0` in steps of `1/4096` including both sides of every seam. It lives in **media**, because core cannot see the pipeline and the pipeline's crate can see both. *Fails:* the same comparison against a deliberately mis-seamed transcription (`linear <= 0.018`) differs at `0.018`, so the sweep is known to be able to see a one-branch error.
13. **`cc7_the_chart_band_is_achromatic_and_the_primaries_band_has_no_red`** — key `raster.a1_guard`. §3.5(7): every chart patch `R == G == B`; no `[255,0,0]` anywhere; the derived `product_red` qualifier centre is more than `hue_width + softness` from every primary's grade709 hue. **This is the fixture that stops a later tidy-up from putting the red primary back and silently breaking (d).** *Fails:* re-inserting `[255,0,0]` trips it.
14. **`cc7_camera_sources_differ_from_the_reference_at_every_neutral_patch`** — key `sources.non_vacuity`. §3.5(1). *Fails:* cam A against itself measures 0.
15. **`cc7_log_source_is_not_the_base_scene`** — key `sources.log_signature`. §3.5(2). *Fails:* cam A fails both percentile bounds.
16. **`cc7_tracked_source_moves_and_occludes`** — key `sources.tracking`. §3.5(3) over the eleven sampled frames, `product_red` at every one except **47**. *Fails:* a source with the square drawn on every frame reports `product_red` at 47 and is asserted to.
17. **`cc7_tracked_square_never_covers_the_static_patch_row`** — key `sources.tracking`. §3.5(4).
18. **`cc7_ffv1_round_trip_is_byte_exact`** — key `sources.lossless`. §3.5(5). *Fails:* a `libx264 -crf 23` mux of the same planes is asserted **not** byte-exact, so the FFV1 claim is a measurement.
19. **`cc7_mixed_camera_match_meets_the_neutral_spread_and_luma_budgets`** — key `budgets.match`. §4(a)(2)+(3), the (a) exit gate; measured spread **2** codes against a budget of **5**, and luma **−1 381 567** millionths against **5 000 000**, both recorded. *Fails:* items 20, 20b and 21.
20. **`cc7_a_the_unmatched_candidate_exceeds_the_neutral_spread_budget`** — key `budgets.match.failing_direction`. Unmatched cam B at **6** codes — the measurement that forced the budget from 6 to 5 (A15), and the reason this fixture exists separately from item 20b.
20b. **`cc7_a_the_unrecoverable_candidate_exceeds_the_neutral_spread_budget`** — key `budgets.match.failing_direction`. Corrected C2 at **19** codes, 3.8× over.
21. **`cc7_a_the_unmatched_candidate_exceeds_the_luma_mean_budget`** — key `budgets.match.failing_direction`. Unmatched B at **−19 904 917**, 3.98× over. Corrected C2 measures **−4 302 267** and is asserted to **pass** this term, so the fixture also proves the spread and the luma gates are not the same gate (R-M10).
22. **`cc7_wrong_balance_clamps_temperature_and_raises_one_range_warning`** — key `budgets.white_balance`. §4(b)(2)+(3): clamp at +100 (raw 232.4), exposure +2 293 unclamped, `delivery_range_excursion` Warning at **22 bp** on the **blue primary** (128 px), per-node `+22 / 0`, `technical_pass == true`, skin `in_band 10 000` reported (was 9 411 — §0.3 R4-m6). *Fails:* `cc7_b_c1_publishes_no_clamp` and `cc7_b_c1_raises_no_range_excursion`.
23. **`cc7_log_inverse_lands_every_patch_inside_the_budget`** — key `budgets.log_inverse`. §4(c)(2), set-wide worst **4** against 12, over the twelve achromatic plus four skin patches. *Fails:* `cc7_c_an_identity_cube_does_not_undo_the_log_curve` at **85** — and the same fixture asserts `chart06` alone reads **1** under the identity cube, so a single-patch gate is proved vacuous rather than merely deprecated.
24. **`cc7_c_the_cube_size_sweep_is_monotone_and_size_seventeen_fails`** — key `log.cube_size_ladder`. §4(c)(3): **13 / 7 / 4** at sizes 17 / 33 / 65, monotone non-increasing, `CC7_LOG_CUBE_SIZE = 65` pinned, the black patch size-independent at 4, and `CC7_LOG_CUBE_BYTES_REPORTED = 7 414 990` under `LUT_MAX_FILE_BYTES`. *Fails:* size 17 at 13 codes, over the budget of 12.
25. **`cc7_product_qualifier_covers_exactly_its_patch_and_changes_nothing_outside`** — key `matte.containment`. §4(d)(1)+(2): `covered == full == 192`, `partial == 0`, outside `0`. *Fails:* `cc7_d_a_qualifier_that_selects_two_patches_is_rejected`.
26. **`cc7_feather_counts_match_the_discrete_pixel_centre_model`** — key `matte.feather`. §4(d)(4), on the **window-only (d2) node** (R-B4; the fixture asserts the committed node stores no qualifier parameter, so a future merge of (d) and (d2) fails here rather than silently measuring `192 / 140 / 52`): `full 140 / covered 252 / partial 112` within `CC7_FEATHER_PARTIAL_TOLERANCE_PIXELS = 4`, and the continuous-area value `76.8` asserted **not** to match, so the wrong model cannot be reintroduced. *Fails:* `cc7_d_feather_zero_has_no_partial_pixels`. **Non-cuttable** (R-M16).
27. **`cc7_warm_look_out_of_gamut_count_is_exact_on_the_deep_shadow_patch`** — key `look.gamut`. §4(e)(2): ROI count `== 192`, `delivery_gamut_excursion` Warning, `technical_pass == true`, whole-raster count and basis points recorded. *Fails:* `cc7_e_the_base_scene_without_the_look_is_in_gamut`.
28. **`cc7_tracked_window_contains_the_square_at_every_sampled_frame`** — key `tracking.containment`. §4(f)(3): the 1.5× window at every surviving sample except the named final keyframe 42, required half-extents **14.77 / 12.88 px** and margins **3.23 / 5.12 px** recorded. *Fails:* `cc7_f_a_window_smaller_than_the_square_loses_containment` — the seeded 1.0× window is 2.77 px short in x.
    - **`cc7_f_the_default_floor_drops_no_sample`** and **`cc7_f2_the_default_floor_does_not_refuse`** (agent) — the two directions that make `CC7_TRACK_MIN_CONFIDENCE_BASIS_POINTS` load-bearing rather than decorative (A13, A14).
29. **`cc7_every_scenario_verifies_at_eight_bits`** and **`cc7_every_scenario_verifies_at_ten_bits`** — key `delivery`. §4(g)(1) for all six canonical documents at **both** depths (A10 — the Ten lane is no longer a cut candidate), CC6's budgets reused verbatim, per-scenario measured value and margin recorded, and the allowed-Info-code set asserted exactly. *Fails:* `cc7_g_a_starved_encode_trips_the_decoded_difference_budget`.
    - **`cc7_canonical_node_stack_matches_the_cpu_reference_on_the_software_lane`** and **`…_on_hardware`** (`#[ignore]`) — key `parity`. §4(a)(5)'s compositor-layer case, `LINEAR_CPU_GPU_*` unchanged, plus the recorded determinism figure `0 / 0 / 0` over 172 800 samples. *Fails:* the CC6 negative control.

**Agent — `crates/kinewright-agent/tests/mcp_server.rs` (`CC7_AGENT_TESTS`)**

30. The six §5.2 scripts, plus **`cc7_the_agent_surface_is_unchanged_by_this_slice`** (§5.4). *Fails:* each script's own typed-refusal column; the surface test fails on any registry or served-count change.

**App — inline `#[cfg(test)]` (`CC7_APP_TESTS`)**

31. The **seven** §6 tests — five builders in `inspector_ui.rs`'s `mod tests`, the (d2) window-only test beside them, and **the (e) test in `look_browser_ui.rs`'s `mod tests`** beside `add_as_new_look_stacks_a_second_node_after_the_first` (`:539-568`), so that `CC7_TEST_SOURCES`' entry for that file is non-empty and the both-direction inventory does not assert an empty set (minor 7) — plus the (f) N/A assertion. *Fails:* each asserts the built batch against the canonical document, so a builder that omits a parameter fails on the document comparison rather than on a count.

**Eval — `crates/kinewright-agent/src/eval.rs` and `src/bin/kinewright-eval.rs` (`CC7_EVAL_TESTS`)**

32. **`published_v6_manifest_tracks_the_color_workflow_suite`** (§7.7, including the generated-vs-checked-in `ground-truth.json` byte equality); **`cc7_color_workflow_suite_is_a_packaged_benchmark`** (the EVALS.md-overwrite guard); **`cc7_a_v5_result_serialises_byte_identically_without_measurements`** (R-M3 — the replacement for draft v3's un-writable "pre-CC7 record parses" test, since `EvalResult` is `Serialize`-only); **`cc7_color_evidence_is_computed_where_the_analysis_is_alive`** (R-B1 — `EvalOutcome.color` is `Some` after a run with a colour fixture and `None` for a v1–v5 fixture, so the plumbing is exercised and the other suites are proved untouched); **`cc7_a_fixture_project_path_reaches_the_server`** (R-B2 — `project_path: Some(_)` makes `import_lut_asset` succeed and `None` still refuses `project_not_saved`, both directions); **`cc7_the_blind_package_discloses_no_machine_provenance`** (§8.5, scanning the listing **and** the form bytes, with both leak directions); **`cc7_human_review_v1_files_still_load_and_score`** (§9.4); **`cc7_accepted_requires_every_question_answered`** (§8.3, both directions); **`cc7_score_review_resolves_a_blind_form_before_binding`** (§8.2 — a `blind/review-form.json` scores through the key, and a `blind_id` absent from the key is a typed error).

**Inventory (`CC7_INVENTORY_TESTS`)**

33. **`cc7_manifest_declares_every_required_fixture_and_constant`** and **`cc7_declared_test_names_exist_in_their_source_files`** — §11.3.

**No tolerance may be used to excuse a missing or wrong delivery tag, a fabricated measurement, a raster that is not full-resolution claiming to be a delivery reference, a scenario whose two paths were compared against different documents, a blind package that leaks its provenance, a check with no failing case, or a budget no measurement approaches.**

### 11.3 `cc7_manifest.json`

`crates/kinewright-media/tests/fixtures/cc7_manifest.json`, `include_str!`'d and asserted key by key (CC6's pattern at `cc6_fixtures.rs:2549-2552`). **It is authored after §12 step 5 and must contain no unresolved placeholder — every threshold key holds a measured number**, and the **key count is asserted** so a constant cannot be added without declaring it.

Structure: `contract`, `contract_token`, `manifest_version: 1`, then

- **`scenarios`** — one object per scenario: `id`, `title`, `clips`, `frames`, `canonical_operations`, `human_question` (`null` for (c)), `person_path`;
- **`raster`** — `size`, `fps`, `frames`, `regions` (pixel rect + `NormalizedRoi` per §2.3.3 and §2.5), `populations` (five entries summing to 57 600), `a1_guard` (the achromatic-chart and no-red claims), the basis-point conversion rule as prose, and **`luma_percentiles`** — `{ first, median, ninety_ninth }` in **16-bit** codes from `measure_scopes`, for **every scenario's canonical document at its sample frame** (R-M15). The roadmap's threshold paragraph requires the evidence to record first/median/99th luma percentiles, `analyze_color_shot` already returns them, and CC7 records them for all six scenarios rather than only for the (c) carrier;
- **`patches`** — `chart` (twelve achromatic codes), `primaries` (five codes), `cam_a`, `cameras` (B/C1/C2), `qualifier` (the nine derived values and the sample statistics);
- **`log`** — `curve` (offset, span, anchors, the twelve stored codes), `round_trip` (§2.4.2's error column and the two structural floors), `signature` (`{ field: "scope_statistics.luma", unit: "sixteen_bit_code", scale: 257, first_percentile_min: 5140, p99_max: 51400, eight_bit_prose: [20, 200], carrier: [7196, 31611, 42919], cam_a: [2570, 29555, 62194], wrong_field: "mean_code_values.luma" }`), `cube_size: 65` with `size_is_pinned_not_selected: true`, `cube_size_ladder` (`{17: 13, 33: 7, 65: 4}` plus `monotone_non_increasing: true`), `cube_bytes: 7414990`, `identity_cube_worst: 85`, `black_patch_code: 4`, `primary_code_not_gated: 5`, `cube_sha256`, `clamp_kept: true`;
- **`tracking`** — `path` formula, `amplitudes: [100, 40]`, `square_size: 24`, `occlusion_frames: [43, 47]`, `range: [0, 48]`, `step_frames: 5`, `search_radius_percent: 10`, `max_width: 256`, `sample_frames: [0,4,9,14,18,23,28,32,37,42,47]`, `expected_low_confidence_frames: [47]`, `analytic_centres_basis_points` (eleven pairs), `seeded_window_half_sizes_basis_points: [375, 667]`, `containment_window_half_sizes_basis_points: [563, 1000]`, `confidence_floor: 8500`, `occluded_confidence_max: 7411`, `clean_confidence_min: 9740`, `worst_raw_observation_error_basis_points: 49`, `containment_required_half_extents_pixels_hundredths: [1477, 1288]`, `containment_worst_margin_pixels_hundredths: [323, 512]`, `f2: { range: [0, 48], step_frames: 47, samples: [0, 47], occluded_confidence: 7309 }`, `no_re_acquisition_drift_basis_points: 5176`, `radius_10_and_25_identical: true`, `owner: "kinewright-agent"` for the observations and confidence;
- **`transcriptions`** — the three functions `cc7_scenarios` transcribes (`encode_bt709`, `decode_display709`, `grade709_decode`), the module and contract section that **owns** each, the tolerance `1e-6`, and the sweep bounds asserted by §11.2.12b (R-M2);
- **`thresholds`** — one key per §2.6 constant, asserted **equal to the code constant** by `assert_manifest_i64`, with the key count asserted, plus `distinctness` and `delivery_allowed_info_codes`;
- **`budgets`** — per scenario and per term, `{ budget, measured, margin }`, plus a `measurement` provenance block naming **OS, lane, adapter, source generator, FFmpeg build, libavcodec/libswscale versions, x264 core, rustc version, and commit**; plus `failing_direction` mirroring §4.2; plus `delivery` recording, per scenario and per depth, CC6's constant, the CC7 measured value, and the margin, with the note **"CC6 owns these constants; CC7 measures against them and never re-baselines one"**; plus `reported_not_gated` for the RGB extremes, the C2 residual spread, the C2 skin band, and the whole-raster gamut count;
- **`performance`** — per-scenario export and verify wall time, the measured `~3.75 s` per 60-frame export + verify on llvmpipe, and `CC7_DELIVERY_LEG_BUDGET_SECONDS_LINUX = 90`;
- **`eval`** — `benchmark_id`, `task_ids`, `assertion_variants` (the eight of §7.5, including `TrackKeyframesMatchExpected` and **not** a tool-log variant), `budgets` per task, `published_manifest_keys`, `color_eval_evidence_fields`, `project_path_required_for: ["c3"]`, and `delivery_bit_depth_literal_sites` (the ~5 edited spec literals, R-B7);
- **`review`** — `schema_version: 2`, `blind_id_derivation`, `blind_form_keys` (the exact key set of `blind/review-form.json`), `blind_key_location: "run_root"`, `leak_test_needles` (task ids, run id, benchmark id, `agent`/`person`/`model`/`harness`/`passed`/`assert`, every `cc7_scenarios` parameter name and canonical value), `not_blinded: ["scenario_identity"]`, `not_applicable_dimensions`, `questions` per scenario, `human_gate`;
- **`scorecard`** — `objective_assertion_count` / `human_question_count` per scenario, `person_agent_parity`, `generalization_matrix`;
- **`m36`** — `registry_tools: 124`, `registry_bytes: 1280060`, `served_tools: 7`, `served_bytes: 5660`, `changed_by_cc7: false`;
- **`external_owners`** — the fixtures CC7 **cites** rather than duplicates: `cc1_fixtures.rs:3139/3146/3153`, `:3294-3296` (log refusals), `cc4_fixtures.rs:3516` (relocation), `mcp_server.rs:1351` / `:1439` (project-path handle), `cc5_fixtures.rs:1149-1172` (containment), `cc6_fixtures.rs:2346` (verified export record), CC6 §6.3's budget constants;
- **`required_fixtures`** and **`manifest_self_test`** naming the two inventory tests.

**Inventory arrays**, in `cc7_fixtures.rs`, each a pinned `[&str; N]` compared as a **sorted set equality in both directions** so a test that exists but is undeclared fails exactly as loudly as a declared name that does not exist:

```text
CC7_MEDIA_TESTS  CC7_CORE_TESTS  CC7_AGENT_TESTS  CC7_APP_TESTS  CC7_EVAL_TESTS
CC7_INVENTORY_TESTS  CC7_EVIDENCE_FIXTURES  CC7_EXTERNAL_OWNERS
CC7_TEST_SOURCES: [(&str, &str); 8]
```

`CC7_TEST_SOURCES` maps each workspace-relative path to its `include_str!` text — a **compile-time** dependency, so renaming a test in another crate rebuilds this fixture and fails it, **and the reason the whole inventory is §12's final step 9b** (R-M12): it reaches into step 4's, step 7's and step 8's files, so it cannot be authored before them. Its eight entries: `cc7_fixtures.rs`, `cc7_sources.rs`, `../../kinewright-core/tests/cc7_core.rs`, `../../kinewright-agent/tests/mcp_server.rs`, `../../kinewright-agent/src/eval.rs`, `../../kinewright-agent/src/bin/kinewright-eval.rs`, `../../kinewright-app/src/inspector_ui.rs`, `../../kinewright-app/src/look_browser_ui.rs`. `declares_test` requires a real `#[test]` / `#[tokio::test]` attribute above `fn name(`, so a doc-comment mention does not count; `cc7_test_source` panics on an invented path; every declared name must start with `cc7_` **or** be named explicitly in the inventory (the `published_v6_…` and app-builder names are the explicit ones).

**The `uses_outside_prose` guard.** `uses_outside_prose(source, needle)` (`cc6_fixtures.rs:2524-2541`) counts a needle as used when a non-comment line contains `needle(` or `("needle")`. `cc7_manifest_declares_every_required_fixture_and_constant` asserts that **`cc7_fixtures.rs`, `cc7_sources.rs`, `cc7_core.rs`, and every app source in `CC7_TEST_SOURCES` never reach for `fixture_gpu_or_skip` or `KINEWRIGHT_GPU_TESTS_MAY_SKIP`** — **and the needles must be written as an array literal**, `const CC7_FORBIDDEN_HELPERS: [&str; 2] = ["fixture_gpu_or_skip", "KINEWRIGHT_GPU_TESTS_MAY_SKIP"];`, which is normative (R-M11). The guard matches `needle(` **or** `("needle")`, so a single-argument helper call such as `assert_forbidden(source, "fixture_gpu_or_skip")` contains `("fixture_gpu_or_skip")` and **self-matches**; CC6 escapes only because its needles sit in an array literal, where each is preceded by `[` or `, ` and never by `(`. Draft v3's stated reason — "the guard requires a call or string-literal shape" — was not a reason, and the array-literal form is the actual mechanism. The **only** file permitted to use `KINEWRIGHT_GPU_TESTS_MAY_SKIP` is `tests/mcp_server.rs`, and there it must appear inside a branch that asserts a typed code (§5.3).
---

## 12. Implementation order

**Size, estimated.** CC5 landed 28 974 insertions; CC6 landed 23 254 insertions / 424 deletions. CC7 writes no engine and no UI, but it writes six scenarios in five crates and one real harness change. Estimate **13 000–18 000 insertions**, split roughly: `cc7_scenarios.rs` 1 500 (it now carries its own f64 transcriptions, R-M2), `cc7_sources.rs` 1 000, `cc7_fixtures.rs` 3 700, `cc7_core.rs` 1 600, `cc7_manifest.json` 1 100, `mcp_server.rs` +1 800, eval (`eval.rs` + `kinewright-eval.rs`) +3 200 (the `ColorEvalEvidence` plumbing, `project_path`, the blind package), app inline tests +900, `benchmarks/auto-edit/v6/` 400, docs 800. The measured number replaces this estimate in §0.3.

**Cut order (R-M16).** Draft v3's two cut candidates were exactly two cells of the roadmap's colour evaluation matrix — "matte edges" on the Product-and-skin row and "asset portability" on the Creative-look row — so cutting either left a roadmap-required objective check undelivered. **Both are therefore non-cuttable.** The cut order is:

1. **The `EvalMeasurement` block** (§7.6). Fallback: the numbers stay in the `AssertionResult.detail` strings, exactly as every existing suite reports them today. Nothing is unmeasured; the evidence is just less structured.
2. **The eval suite's `c4`, `c5` and `c6` tasks.** Fallback: `color-workflow-v6` ships `c1`–`c3`, and `c4`–`c6` are recorded as owed in §13 and in `docs/EVALS.md`'s suite row. The *scripted* mcp tests for all six scenarios are **never** cut, so no scenario loses its end-to-end evidence — only the model-driven lane narrows.

**Never cut**, because they are the exit gate: the six scenario sources (§3), the six agent end-to-end tests (§5), the person-path tests for (a)–(e) (§6), (d2) and the (e) portability check (§4(d)(4), §4(e)(4) — matrix cells), the blind package and its leak test (§8.2/§8.5), and the `technical_pass` / `within_budgets` / allowed-Info gates at **both** depths (§4(g)(1)).

1. **Core authority.** `crates/kinewright-core/src/cc7_scenarios.rs` in full (§2), including its **own f64 transcriptions** of `encode_bt709`, `decode_display709` and `grade709_decode` with the named-owner comment (R-M2); `lib.rs` registration and re-export; `crates/kinewright-core/tests/cc7_core.rs` items 1–11. *Size ≈ 3 100.*
2. **Media generators.** `crates/kinewright-media/src/cc7_sources.rs` (§3) with A11's module doc and §11.0.1's source-content exemption, and the seven non-vacuity fixtures of §3.5. *Size ≈ 1 700.*
3. **Media gates.** `cc7_fixtures.rs` items 19–29 plus item 13's A1 guard and item 12b's transcription cross-check, written against the §2.6 constants, every one of which holds a measured number. *Size ≈ 3 700.*
4. **Agent scripts.** The six `cc7_` tests in `tests/mcp_server.rs` (§5), the branch-server project-path handle for (c), and the surface-unchanged test. *Size ≈ 1 800.*
5. **Measurement (A20's remainder, Q1, Q6, Q7).** Probe-1, probe-2 and probe-3 have measured (a), (b2), (c), (d), (e), (f), (f2) and (g). What is still owed, on the **amended** scene, before the manifest is authored: **C1's proposal and the (b1) residual** (Q1 — the implementer measures these in the media fixture, because (b1) now reuses a budget of 5 rather than 6); the **(b2) per-node `range_basis_points_delta`** (Q7 — expected `+116`, **confirmed, never pinned from the report**); the **(a) skin `in_band` and skin-row chroma** numbers; the **per-scenario luma percentiles** of R-M15; the **(g) starved-encode** failing direction; and **the whole CC7 media suite's wall time** (Q6). Every such cell is marked "(P1; re-measured at §12 step 5)". **Steps 6–9b must not start against unconfirmed budgets.** Windows is measured by the CI job (P10).
6. **Fixtures and manifest.** `tests/fixtures/cc7_manifest.json` authored from step 5's numbers, including `raster.luma_percentiles` per scenario (R-M15); the budget and distinctness assertions (items 9, 10). **The inventory arrays and the both-direction declared-name assertion do NOT land here** — see 9b. *Size ≈ 1 500.*
7. **Person path.** The **seven** app tests (§6) — five builders plus the window-only (d2) test in `inspector_ui.rs`'s `mod tests`, and **the (e) test in `look_browser_ui.rs`'s `mod tests`**, beside `add_as_new_look_stacks_a_second_node_after_the_first` (`:539-568`), so that `CC7_TEST_SOURCES`' entry for that file is non-empty (minor 7). *Size ≈ 900.*
8. **Eval suite, harness plumbing and the blind package.** This is a **shared-runner change**, not a suite addition, and is sized as one: `eval.rs` — `ColorEvalEvidence`, `original_document` and `color` on `EvalOutcome`, the measurement block computed inside `run_eval_with_artifacts` (R-B1), the eight assertion variants including `TrackKeyframesMatchExpected`, `EvalMeasurement`, `PreparedFixture.project_path` and the `set_project_path` call (R-B2), `EvalDeliverableSpec.delivery_bit_depth` **plus the edit to every existing struct literal** (~5 sites in `kinewright-eval.rs`: the v2/v3/v4/v5 suite constructors, e.g. `:1679-1691`) and the two spec-fed `Eight` sites `eval.rs:992` / `:1132` (`:6157` out of scope, R-M13), human-review v2, the `blind/` writer, `blind/review-form.json`, `blind-key.json`, and `--score-review`'s key resolution **before** `verify_review_artifact_bindings` (R-B8); `kinewright-eval.rs` — `color_workflow_suite`, the `eval_suite` arm, `is_packaged_benchmark`, `print_usage`, the six fixture builders, `write_review_package`'s blind output; `benchmarks/auto-edit/v6/`; the eval unit tests (item 32). *Size ≈ 3 600.*
9. **Docs.** This file promoted to `docs/CC7-WORKFLOW-EVALUATION.md`; `docs/ROADMAP-AND-WORKFLOWS.md` (status paragraph, the CC7 table row, and the next-slice line → "CC7 complete; the colour programme table is complete; later programmes (HDR/RAW/ACES) remain deliberate deferrals"); `CHANGELOG.md`; `docs/M36-AGENT-RUNTIME-EFFICIENCY.md` (two appended rows, registry unchanged, stated); `docs/EVALS.md` (run line, usage string, seed-suite row, baseline "pending real-harness run"); `benchmarks/auto-edit/v6/README.md`. *Size ≈ 800.*
9b. **Inventory, last (R-M12).** `CC7_MEDIA_TESTS`, `CC7_CORE_TESTS`, `CC7_AGENT_TESTS`, `CC7_APP_TESTS`, `CC7_EVAL_TESTS`, `CC7_INVENTORY_TESTS`, `CC7_EVIDENCE_FIXTURES`, `CC7_EXTERNAL_OWNERS`, `CC7_TEST_SOURCES`, the `uses_outside_prose` guard, and **`cc7_declared_test_names_exist_in_their_source_files`** — the both-direction assertion — plus `cc7_manifest_declares_every_required_fixture_and_constant`'s test-name half. *Size ≈ 400.*

**Dependency sentence.** Steps 1 → 2 → 3 are strictly ordered. Step 4 depends on 1 and 2. Step 5 depends on 3 and 4. Step 6 depends on 5. Steps 7 and 8 depend on 1 and 2 and may proceed in parallel with 6, but **neither may assert a budget before step 5 lands**, and step 8's `benchmarks/auto-edit/v6/ground-truth.json` may not be generated before step 5 confirms the remaining canonical values. **Step 9b is last and depends on 4, 6, 7 and 8**, because `CC7_TEST_SOURCES` `include_str!`s `mcp_server.rs` (step 4), `inspector_ui.rs` and `look_browser_ui.rs` (step 7) and `eval.rs` / `kinewright-eval.rs` (step 8) at **compile time**, and the both-direction assertion fails on any `cc7_*` test that exists but is not yet declared. Draft v3's "steps 7 and 8 may proceed in parallel with 6" was true of the *fixtures*; it was false of the *inventory*, which is why the inventory is now its own final step.

## 13. Explicit deferrals

Each names why it is a slice and not a flag.

- **A noise or grain measurement.** The matrix's "noise warnings" for scenario (b) have **no** backing measurement: grep across the three crates finds only an unrelated ffmpeg eval filter and a test frame generator. A noise metric needs a definition (temporal? spatial? per-channel? demosaic-aware?), a reference population, its own analytic fixture, and a threshold that survives a codec. **Not free:** the only honest deterministic substitute available is `noise=alls=N:allf=t`, which is uniform pixel noise, not sensor character, and measuring it would measure the filter. (b2)'s compromise is carried instead by the measured residual spread of 17 codes and the skin band at 9 411 bp.
- **Kelvin or mired white balance and chromatic adaptation.** `temperature_percent` is a ±0.1 %-per-percent diagonal RGB gain (`CC1:322-328`), deferred at `CC1:533`. A Kelvin control needs a white-point model, a chromatic adaptation transform, a UI unit change, a migration for every stored `temperature_percent`, and a re-derivation of `match_parameters`' first-order step. **Not free:** (b2)'s clamp — raw 232.4 against a bound of 100 — is *evidence for* that slice, and CC7 records it rather than pre-empting the fix.
- **Log, LogC, Log3G10, and camera-native source profiles.** Adding them means new `ColorSourceProfile` variants, new `decode_transfer` arms, built-in technical transforms, and **amending four existing CC1 refusal fixtures** plus `docs/CC1`'s contract. **Not free:** scenario (c) proves the *workflow* — a BT.709 carrier normalised by an imported `.cube` — without touching CC1's refusal.
- **An exact black-point inverse for the log curve.** `v = 0` inverts to `2^−8` linear, code 4, at every lattice size (§2.4.2, A2). Fixing it means a curve with a defined toe and a matching inverse, i.e. a real camera-transform contract. **Not free:** the 4-code floor is recorded as a property of the curve and absorbed by `CC7_LOG_INVERSE_MAX_CODE = 12`, rather than papered over by excluding the patch.
- **A matte edge-quality metric.** `MatteCoverageStatistics` (**`crates/kinewright-core/src/media.rs:673-726`** — core) offers `partial_pixel_count` and the 16-bucket `coverage_histogram` and nothing else; no perimeter, gradient, feather-width, or edge-contrast measure exists. **Not free:** an edge metric needs a definition, a resolution-independent normalisation, and a GPU/CPU parity gate of its own; (d2) measures the feather *band population*, which is geometry, not edge quality.
- **A per-sample track-lost marker.** Below the confidence floor a sample is dropped into `low_confidence_samples` and the gap is bridged by Linear interpolation with **no marker** (`server.rs:4617-4630`). **Not free:** a marker is a response-shape change, a keyframe-semantics change, and a UI affordance the app has no place for; §4(f)(1) gates the *identity* of the dropped samples instead.
- **Occlusion re-acquisition, and therefore any gate that spans an occlusion (A12).** `track_matte_window` has **no re-acquisition**: probe-2 measured that from the first post-occlusion sample onward the observed centre is **frozen** at its pre-occlusion value while every one of those samples reports `confidence_basis_points = 10 000` — up to **5 176 bp (165 px)** from the subject by frame 74. The window holds its last measured position through the occlusion and does not cover the hidden subject at any floor or amplitude (measured: at frame 45 the held window covers `x 219.9..243.9` while the square is at `x 179..203`, no overlap at all). This is the tool's documented `MATTE_TRACKING_BOUNDARY` — "normalized SAD template match on composited thumbnails; … no occlusion handling" — not a defect. **Not free:** re-acquisition needs a search that is not seeded from the previous sample, a re-detection criterion, and a confidence model that can say "lost" rather than "perfectly matched flat surround" — a tracking slice, not a flag. CC7's consequence is normative and narrow: **the tracked range ends at the occlusion**, the sample inside it is reported as low-confidence rather than as an observation, and **no CC7 gate spans an occlusion**. A4's conditional fallback (drop the sub-gate) did **not** trigger: the measured separation is 2 329 bp, above its 2 000 bp bar.
- **A person path for scenario (f).** `matte_track_button_enabled() -> false` (`inspector_ui.rs:2868-2872`) because the app has no agent-tool call path. **Not free:** enabling it means either an in-app tracker (a second implementation of `track_region`, with its own parity gate) or an app→agent call path (a new architectural boundary). §6 records 5/6 rather than hiding the gap.
- **A live-UI harness.** `KinewrightApp::new` is private and there is no `crates/kinewright-app/tests/`. **Not free:** a harness needs a constructible app, an injectable media engine, a deterministic frame pump, and a way to assert a rendered frame — four things that do not exist.
- **A recorded cross-platform decoded delta.** Platform consistency is "the same budgets pass on both CI OSes". **Not free:** a recorded delta needs an artefact-comparison subcommand, a place to store one OS's encode for the other to read, and a budget of its own that is not a codec tolerance inherited from another lane — which `ROADMAP:516-517` forbids.
- **A colour rating dimension.** `HumanRatingDimension` stays at six. **Not free:** a seventh variant bumps the wire schema for every historic `human-review.json`, changes `summarize_human_review`'s "every dimension when `accepted` is set" rule for **all** suites, and invalidates M37/M40's recorded means. §8 carries the colour judgement in `questions` instead, which is additive and defaults away.
- **The model-driven lane for (d), (e) and (f), if §12's second cut is taken.** `color-workflow-v6` would ship `c1`–`c3` and `c4`–`c6` would be owed. **Not free:** the scripted mcp tests still prove all six scenarios end to end at the tool level, so what is lost is the evidence that a *model* can choose the parameters for a secondary, a look and a track — which is the whole point of the suite and the reason this is the last thing cut, not the first. If taken, the deferral is recorded here with the date, in `docs/EVALS.md`'s suite row, and in `benchmarks/auto-edit/v6/README.md`.
- **Structured `EvalMeasurement` evidence, if §12's first cut is taken.** The numbers stay in `AssertionResult.detail` strings, exactly as every existing suite reports them. **Not free:** a string is not queryable and a later slice comparing runs would have to parse prose — which §10.4 forbids everywhere else — so the fallback is explicitly a *temporary* shape, not a design.
- **Blinding the scenario identity.** §8.0 states it: the question names the scenario, and a chart raster is recognisable. **Not free:** hiding it would mean asking a generic question, which destroys the matrix's per-row value, or synthesising decoy artefacts, which is fabrication. CC7 blinds the machine provenance and says plainly what it does not blind.
- **ΔE2000, VMAF, SSIM, and every perceptual metric**, unchanged from CC6 §13; **HDR, BT.2020, PQ, HLG, ACES, OCIO, RAW**, unchanged from CC6 §13 and CC1 §7; **a second delivery lane, codec, or container**, unchanged from CC6 §13.

CC7 is complete only when a colourist can watch six ordinary jobs — match two cameras, rescue a mis-balanced take, normalise a flat log clip, isolate a product without touching skin, apply a look, and follow a moving subject through an occlusion — go through Kinewright by hand *and* by agent *and* by model, see every objective claim discharged by a build that runs on both operating systems with no model and no network, and be asked only the six questions a machine has no business answering.

---

## 14. Risks

- **The cells probe-2 did not re-measure are still pre-amendment predictions (A20).** Probe-2 re-ran (a), (b2), (d), (e), (f) and (f2) on the amended scene and **overturned two constants and one prose claim in the process** — the spread budget 6 → 5, the reported residual 17 → 19, and (b2)'s clipping population from 128 blue-primary pixels to 672 across three primaries, the white patch and the ramp. That is the base rate for this hazard, and it has now bitten a third time: probe-3 found that the (c) signature constants were written in **8-bit** while the tool publishes **16-bit**, which would have made one gate pass on everything and the other fail on everything (A21). Probe-3 closed (c) — and refuted draft v2's own guess that the six new intermediate greys were the risk; they are the best-behaved patches in the set. What remains un-re-measured is smaller and named in §12 step 5: C1's proposal, the (b1) residual, the (b2) per-node delta, (a)'s skin numbers, and (g)'s starved encode. Mitigation: every such cell is marked "(P1; re-measured at §12 step 5)", §12 step 5 is a hard barrier, and the manifest may hold no unresolved placeholder. Fallback if a budget moves: re-baseline the CC7 constant **once**, with the amended-scene measurement recorded beside the pre-amendment one — never widen after a red build.
- **The tracking gates rest on a 2 329 bp separation and a floor with 1 089 bp of headroom.** `CC7_TRACK_MIN_CONFIDENCE_BASIS_POINTS = 8 500` sits between a measured occluded maximum of **7 411** and a measured clean minimum of **9 740**; both populations are SAD scores over composited thumbnails, so a change to the compositor, the thumbnail scaler, or the `product_red`/surround codes moves both. The margin is comfortable but it is not a 2× ratio, and §4.1 note 5 says so rather than printing one. Mitigation: both populations are recorded in the manifest (`occluded_confidence_max`, `clean_confidence_min`), so a future move is diagnosable as a shift in one or the other rather than as "the tracking test went red"; the two failing-direction fixtures (`cc7_f_the_default_floor_drops_no_sample`, `cc7_f2_the_default_floor_does_not_refuse`) prove the floor is load-bearing in both directions. Fallback: re-pin the floor once from the re-measured pair, with both sides recorded — never widen it after a red build.
- **The smoother, not the tracker, is the thing most likely to fail a badly written tracking gate.** `MATTE_TRACK_MAX_STEP_BASIS_POINTS = 800` is reached once on this path (the `4 → 9` segment, raw Δx 898 bp clamped to 800, self-correcting at the next sample, net ≤ 98 bp), and the three-sample median filter puts the **final** keyframe 746 bp off whenever the last sample is dropped. Draft v2 proposed slowing the motion to avoid the clamp; probe-2 measured that the slower variant is worse on every gated term and that the clamp costs two orders of magnitude less than a containment failure. Mitigation: A17 — every gate reads `observations[]`, the containment gate excludes the named final keyframe 42, and both facts are stated in §4(f) rather than discovered by a maintainer reading a red build.
- **CI time.** Probe-1 measured one 320 × 180 60-frame export + verify at **≈ 3.75 s** on llvmpipe (export 3 328.5 ms, verify 422.5 ms; verify cost is frame-count-independent because it is always five sampled frames). Six scenarios × two depths ≈ **45 s**, plus source synthesis and the working/monitor proofs. `CC7_DELIVERY_LEG_BUDGET_SECONDS_LINUX = 90` is a 2× bound recorded in the manifest, not a hard gate; there is no `timeout-minutes` in `ci.yml`, so the failure mode is slow rather than red. Fallback: take the cut order.
- **Windows first run.** The Windows CI job uses a **different** FFmpeg package (`System233/ffmpeg-msvc-prebuilt ffmpeg-8.0.1-r3`) from the Linux pin (`mifi/ffmpeg-builds 8.0-1`). Every CC7 budget is a Linux measurement until that job runs; the most exposed is the 8-bit luma mean at **1.06×** on scenario (e) (§0.3 C-E8; 2.16× was the pre-A1 figure), the tightest margin in §4.1, and it is a CC6 constant CC7 must not touch. Mitigation: R5's rule — one constant, re-baselined once with a per-OS note if Windows exceeds it, **never** a per-OS constant. Nothing in §4 skips on Windows; `fallback_gpu()` fails loudly.
- **The eval schema change ripples through the baseline tests.** `EvalResult` gains a field, `EvalDeliverableSpec` gains a field, `HumanReviewFile` gains a version. Five `published_vN_…` tests pin assertion counts, SHAs, and human status against frozen JSON. The real compile-time hazard is narrower than draft v3 claimed (minor 9): none of the five `published_vN_…` tests serializes or hashes a live `EvalResult` — they parse checked-in JSON key by key — but `published_v5_manifest_tracks_both_real_footage_families_and_fixture_packs` (`kinewright-eval.rs:4271`) uses **exhaustive `matches!` field lists** over existing `EvalAssertion` variants, so adding a field to an **existing** variant would break it. **CC7 adds only new variants and touches no existing one**, so that test is safe by construction. Mitigation for the rest: `measurements` is `skip_serializing_if` so a result without it serialises byte-identically (§11.2.32); `EvalDeliverableSpec`'s new field is a compile-time edit at ~5 literal sites, which fails loudly rather than silently; `AssertionResult` and `render_scoreboard` are deliberately untouched. Fallback: carry the measurements in a **sibling** `measurements.jsonl`, which costs a second file and loses the per-record binding but changes no existing shape.
- **The A1 guard is the only thing keeping (d) and (e) exact.** Both gates are exact-count gates that were unreachable before A1 and are reachable only while the red primary stays out and the chart stays achromatic. Mitigation: §11.2.13 asserts both properties directly and asserts the qualifier's hue distance from every primary, so a well-meaning "restore the CC6 chart" change fails a named fixture rather than a count nobody can explain.
- **No CC7 fixture asserts an adapter string (Q5).** §4 mandates `fallback_gpu()` in the default lane and `emit_evidence` **records** the adapter, but nothing compares it: on Windows the adapter is not `llvmpipe`, and a fixture that asserted one would go red on the OS it was written to protect. The only adapter assertion in the tree is `fallback_gpu()`'s own Linux-only `lavapipe|llvmpipe` check (`cc1_fixtures.rs:1494-1520`), which CC7 calls **unchanged** and does not extend. This is stated as a negative so a later contributor does not add one.
- **CI cost is budgeted for the whole media suite, not just delivery (Q6).** Draft v3 budgeted only the delivery leg. The measured pieces: one 320 × 180 60-frame export + verify is **≈ 3.75 s** (probe-1 P7), so six scenarios × two depths ≈ **45 s**; probe-3 measured the entire (c) section — two encodes, two managed decodes, a GPU proof pair and four cubes including the 7.4 MB 65³ write — at **0.99 s**; CC6's own `cc6_manifest.json` `performance` block records a 1080p working proof at **1 798.7 ms**, a colour QC at **4.9 ms**, an export at **15 577.3 ms** and a five-frame verify at **12 679.9 ms** on the same lane, all of which CC7 avoids by staying at 320 × 180. **The budget is 180 s on Linux for the whole CC7 media suite** — `CC7_DELIVERY_LEG_BUDGET_SECONDS_LINUX = 90` for delivery plus 90 s for source synthesis, the byte-exact round-trip decodes, the working and monitor proofs and the cube write — **measured at §12 step 5** and recorded in the manifest's `performance` block. There is no `timeout-minutes` in `ci.yml`, so the failure mode is slow rather than red; the budget exists so a regression is visible.
- **The blind package is procedurally defeatable.** A reviewer who opens `blind-key.json`, or who recognises a scenario from its content (a chart raster is distinctive), is unblinded. Mitigation: the leak test bounds what the *tooling* discloses on both surfaces the reviewer touches — the `blind/` listing and the form's bytes (§8.5) — which is what a test can bound; the rest is procedure, stated in `benchmarks/auto-edit/v6/README.md` and in the human gate's wording. §8.0 names the one thing that is deliberately not blinded. An honest limit, not a solved problem.
- **Six scenarios in five crates is a lot of surface for one slice.** The dependency chain in §12 is long and step 5 is a hard barrier. Mitigation: §2's single authority means a scenario is described once; the cut order names two things that can go without touching the exit gate; steps 7 and 8 are parallelisable against step 6.

---

## Appendix A — Measurement provenance

Every number in §2.4.3, §2.6, §4.1, and §4.2 is measured, not inferred. **Three** probes ran, all on Linux on 2026-08-27, all at `99faee3` on a clean tree: **probe-1** measured the pre-amendment scene, **probe-2** the amended (A1) one, and **probe-3** scenario (c) on the amended scene. Every row below carries its attribution, and the cells none of them covered are marked "(P1; re-measured at §12 step 5)" per A20 and listed in row P11.

**Probe-1** (`probe-report.md`, P1–P8):

| Field | Value |
| --- | --- |
| Commit | `99faee3` (`feat: complete CC6 QC and managed delivery`), clean tree |
| OS | Linux (Arch), kernel `7.1.9-arch1-2`, x86-64 |
| Toolchain | `rustc 1.98.0 (88d9e12ae 2026-08-18)` / `cargo 1.98.0 (797e8a9bc 2026-08-05)` |
| GPU lane | default `fallback_gpu()` — `CC_GPU_LANE lane=software_fallback backend=vulkan;adapter=llvmpipe (LLVM 22.1.8, 256 bits);software_fallback=true;gpu_claim=false` |
| FFmpeg (Linux) | `mifi/ffmpeg-builds` **8.0-1**, SHA-256 `c201d31f…5cb1` (`scripts/setup-ffmpeg.sh:27-28`), `n8.0-23-gd1f31a829d-20251022`, libavcodec 62.11.100, libswscale 9.1.100, **libx264 core 165** |
| FFmpeg (Windows CI) | **a different package** — `System233/ffmpeg-msvc-prebuilt` `ffmpeg-8.0.1-r3`, SHA-256 `3399afab…e433` (`scripts/setup-ffmpeg.ps1:12-13`) |
| Probe harness | one scratch file `crates/kinewright-media/src/cc7_probe_scratch.rs`, `cargo test -p kinewright-media --lib -- cc7_probe --nocapture`, default thread count, **removed before the report was filed** |
| Sampling | patch means on a **2-pixel inset** of each patch rect, on the `monitor_proof_for_document` RGBA8 raster |

**Probe-2** (`probe2-report.md`, M1–M6), on the amended scene:

| Field | Value |
| --- | --- |
| Commit | `99faee3`, clean tree |
| OS | Linux (Arch), kernel `7.1.9-arch1-2`, x86-64 |
| Toolchain | `rustc 1.98.0 (88d9e12ae 2026-08-18)` / `cargo 1.98.0 (797e8a9bc 2026-08-05)` |
| GPU lane | default `fallback_gpu()` — `CC_GPU_LANE lane=software_fallback backend=vulkan;adapter=llvmpipe (LLVM 22.1.8, 256 bits);software_fallback=true;gpu_claim=false` |
| FFmpeg | `third_party/ffmpeg` from `scripts/setup-ffmpeg.sh`, `n8.0-23-gd1f31a829d-20251022`, gcc 15.2.0, `--enable-libx264 --enable-libzimg` |
| Probe harness | one scratch file `crates/kinewright-media/src/cc7_probe_scratch.rs` (two tests) plus one scratch `cc7_probe2_tracking` at the end of `crates/kinewright-agent/tests/mcp_server.rs`, driving the **real MCP endpoint**; both removed before the report was filed |
| Runtime | tracking test **8.75 s** wall; media tests 0.74 s + 3.1 s |
| Sampling | patch means on a **2-pixel inset** of each patch rect, on the `monitor_proof_for_document` RGBA8 raster |
| Not measured | **M6** (scenario c) and the (b2) per-node delta — time box; see A20 and §12 step 5 |

**Probe-3** (`probe3-report.md`, M6a–M6c), scenario (c) on the amended scene:

| Field | Value |
| --- | --- |
| Commit | `99faee3`, clean tree |
| OS / toolchain | Linux (Arch) `7.1.9-arch1-2`, x86-64; `rustc 1.98.0 (88d9e12ae 2026-08-18)` / `cargo 1.98.0 (797e8a9bc 2026-08-05)` |
| GPU lane | default `fallback_gpu()` — `lane=software_fallback backend=vulkan;adapter=llvmpipe (LLVM 22.1.8, 256 bits);software_fallback=true;gpu_claim=false` |
| FFmpeg | `n8.0-23-gd1f31a829d-20251022`, gcc 15.2.0, libavutil 60.8.100, `--enable-libx264 --enable-libzimg` |
| Probe harness | one scratch file `crates/kinewright-media/src/cc7_probe_scratch.rs` (one test), `cargo test -p kinewright-media --lib -- cc7_probe --nocapture`; removed before the report was filed |
| Runtime | **0.99 s** wall for the whole section: 2 encodes, 2 managed decodes, 1 GPU proof pair, 4 cubes |
| Method | cubes **written to disk and re-parsed through `parse_cube_lut`**, so the six-decimal quantisation a real `import_lut_asset` applies is inside the measurement; applied through the production `LutNode::apply` at `input_encoding_token = 0`, mix 1.0, on the carrier's real working-linear frame; errors are the max over pixels and channels on a **2-px inset** of each patch |
| Cross-check | the CPU reference on the decoded working frame and the real `monitor_proof_for_document` GPU raster produced **byte-identical** statistics |
| Not measured | nothing in scope |

**No tolerance in this document may be invented, scaled, or inherited from another lane.** The manifest's `budgets.measurement` block records every field above per number, plus the probe that took it, the source generator and its parameters, and the date.

| ID | Question | Answer | Consumed by |
| --- | --- | --- | --- |
| **P1** | (a): the match proposal, post-match neutral spread, chart luma-mean delta, skin band, and render determinism | **Measured on the pre-amendment scene.** Proposal (six-patch grey ROI) `+461 / −42 / +5`, none clamped; spread **2** matched against **6** unmatched; luma delta **+2 166 667** matched, **−14 250 000** unmatched; skin `in_band 10 000` on both A and matched B, `mean_hue 12 257`, spread `121`; skin-row chroma 60.50 (A) → 52.75 (matched B); two renders of the working surface agree **0 / 0 / 0** over **172 800** samples, 0 non-finite. **Superseded by P9 on the amended chart; the skin-band and chroma numbers are re-measured at §12 step 5.** | `CC7_MATCH_NEUTRAL_SPREAD_MAX_CODE = 6`, `CC7_MATCH_LUMA_MEAN_MAX_CODE_MILLIONTHS = 5_000_000`, §4(a)(4), §4(a)(5) |
| **P2** | (b): C1/C2 proposals, clamps, residual spread, range excursion, per-node attribution, skin band | **Measured.** C1 `+1 432 / +77 / −3`, none clamped, residual **2** (from 7). C2 `+2 293 / +100 (raw 232.4) / −20`, temperature clamped only, residual **17**. `delivery_range_excursion` **Warning at 22 bp**, `technical_pass true`, per-node `+22 / 0` with the primary the sole cause and `0 / 0` on cam B. **The clipping pixels are the blue primary patch (128 px), not the chart whites.** Corrected-C2 skin `in_band 9 411`. **Re-confirm on the amended scene (P2).** | §4(b), `CC7_UNRECOVERABLE_RESIDUAL_SPREAD_REPORTED_CODE`, `CC7_C2_SKIN_IN_BAND_REPORTED_BASIS_POINTS` |
| **P3** | (c): the log signature, its **unit**, and the inverse `.cube` error against lattice size | **Measured (probe-1 on the pre-amendment scene, probe-3 on the amended one — identical to the code).** Carrier luma p1/p50/p99 **28 / 123 / 167** against cam A's **10 / 115 / 242** in 8-bit; the fields `analyze_color_shot` actually publishes are **16-bit** (`× 257`) and read **7 196 / 31 611 / 42 919** against **2 570 / 29 555 / 62 194** (A21). Set-wide worst monitoring error over the twelve achromatic plus four skin patches: **13 / 7 / 4** at sizes **17 / 33 / 65**; the six new intermediate greys measure 0–2 at every size; black **4** at every size; white **13 / 7 / 4**; the five primaries **15 / 8 / 5** and are excluded from the gate set. Identity 33³ cube: set-wide **85**, with `chart06` at **1**. `65³` file **7 414 990 B**. `import_lut_asset` needs the project-path handle (`mcp_server.rs:1351`). | `CC7_LOG_FIRST_PERCENTILE_MIN_CODE16 = 5_140`, `CC7_LOG_P99_MAX_CODE16 = 51_400`, `CC7_LOG_INVERSE_MAX_CODE = 12`, `CC7_LOG_CUBE_SIZE = 65`, §4(c) |
| **P4** | (e): the warm look's input encoding, threshold, and out-of-gamut population | **Measured.** Input encoding **display709**, domain `[−1, 2]`, size-17 bake of a per-channel affine formula. Blue crosses zero at `e < 0.074 074 1` (linear `0.016 460 9`); green at `0.037 037`; red never. Whole raster **1 608** px / **279** bp with `delivery_gamut_excursion` Warning and `technical_pass true`; `below_black 348`, `minimum_linear −17 778` millionths. `deep_shadow` alone = **192**, exactly analytic. **Whole-raster count re-measured on the amended scene (P2); the 192 is scene-independent.** | `CC7_LOOK_DEEP_SHADOW_OUT_OF_GAMUT_PIXELS = 192`, `CC7_LOOK_BLUE_ZERO_CROSSING_DISPLAY709_MILLIONTHS`, the reported whole-raster pair |
| **P5** | (d): the derived qualifier, its coverage, and the feather model | **Measured.** Sample `hue_median 35 865` cd, `sat p10 = p90 = 8 728` bp, `luma p10 = p90 = 2 513` bp; qualifier hue `35 865 ± 1 500` softness `1 000`, saturation `7 728..9 728`, luma `1 513..3 513`. Coverage on the **pre-A1** scene `320 / full 192 / partial 128`, the 128 being the red primary — which A1 removes. (d2) at centre `(1687, 4666)`, half `(187, 444)`, `feather 1000`: **full 140 / covered 252 / partial 112**, matching the discrete pixel-centre model **exactly**; the continuous-area model gives 76.8 and is wrong. **Re-confirm `320 → 192` on the amended scene (P2).** | §4(d), `CC7_FEATHER_PARTIAL_TOLERANCE_PIXELS = 4` |
| **P6** | (f): the tool's sample set, per-sample confidence, observation error, containment requirement, and the total-loss recipe | **Measured (probe-2, M1).** Sample set for `0..48` step 5 is `{0,4,9,14,18,23,28,32,37,42,47}`; for `0..48` step 47 it is `{0,47}`; **frame 45 is never a sample** on any grid. Per-sample confidence on the recommended recipe: `10 000` at 0 and 4, `9 869 / 9 859 / 9 862 / 9 870 / 9 865` at 9–28, **`9 740`** at 32 and 37, `10 000` at 42, and **`7 349`** at the occluded 47; occluded maximum across every run **7 411**, clean minimum **9 740**, separation **2 329 bp**. `low_confidence_samples` is **`[]`** at both the 5 000 default and 7 000, and `{47}` at 8 500 and 9 000. Worst clean **raw** observation error **49 bp** (y, frames 14 / 18 / 32) against 104 bp on the rejected `(60, 30)` amplitude. Containment: required half-extents **14.77 px x / 12.88 px y**; the seeded 1.0× window is **2.77 px short in x**; the 1.5× window (563 / 1 000 bp) clears every clean sample by **3.23 / 5.12 px**. The smoothed curve's final keyframe is **746 bp** off when the last sample is dropped (`known_systematic_lag`). **No re-acquisition:** the observed centre freezes from frame 39 to 99 at `(7 051, 6 493)`, **5 176 bp** from the subject at frame 74, every sample reporting `10 000`. `search_radius_percent` 10 and 25 gave bit-identical output. (f2) at `0..48` step 47 refuses `tracking_confidence_too_low` with `surviving_samples 1 / total_samples 2 / minimum_surviving_samples 2`, and does **not** refuse at the 5 000 default. | `CC7_TRACK_MIN_CONFIDENCE_BASIS_POINTS = 8_500`, `CC7_TRACK_TOLERANCE_BASIS_POINTS = 200`, `CC7_TRACK_SAMPLE_FRAMES`, `CC7_TRACK_WINDOW_HALF_*`, §4(f), §13 |
| **P7** | (g): the decoded comparison at both depths, and the CI cost | **Measured.** Canonical two-clip document, samples `0, 29, 59, 89, 119`: 8-bit luma **2 / 1 000 000 / 185 059**, combined mean **499 781**, PSNR **4 148**; 10-bit **1 / 0 / 972**, combined mean **199 455**, PSNR **4 190**; `within_budgets` and `technical_pass` true on both. Tags `bt709×3 / limited`, `mismatches []`, `conforming true`, and **one Info `delivery_tag_not_representable` on `white_point` on every lane**. Reported RGB extremes as in §4.1. Single clip 60 frames: export **3 328.5 / 3 336.7 ms**, verify **422.5 / 418.4 ms**. | CC6 `DELIVERY_*` reused verbatim, `CC7_DELIVERY_ALLOWED_INFO_CODES`, `CC7_DELIVERY_LEG_BUDGET_SECONDS_LINUX = 90` |
| **P8** | Can `cc7_sources` be a `pub` module? | **Measured.** `test_support` is `pub mod` at `lib.rs:27` and every helper it needs is `pub`; the synthesis path uses only `std::process::Command`, `std::fs`, and `std::env`, with no `cfg(test)`-only dependency. `run_ffmpeg` **panics** on a missing binary and a nonzero exit, so the module doc must say test-support-only. | §3, A11 |
| **P9** | The amended (A1) scene: (a)'s proposal and gates, (b2)'s clipping population, (d)'s coverage, (e)'s gamut counts | **Measured (probe-2, M2–M5).** **(a)** cam A chart band `[118.667 ×3]`, spread **0**; cam B proposal **`+477 / −45 / +6`**, none clamped; spread **2** matched against **6** unmatched (which is why the budget is **5**, not 6); luma delta **−1 381 567** matched against **−19 904 917** unmatched. **(b2)** C2 proposal **`+2 410 / +100 (raw +248) / −30`**, temperature clamped only; residual spread **19**; `clamped_basis_points` **116** from **672** over-range pixels, **blue channel only** (`blue.maximum_over_excursion_millionths 41 538`, red and green **0**), split 384 primaries / 128 white achromatic patch / 160 ramp; corrected cam B control measures **0** with no exception; `technical_pass` **true** on both. **(d)** `covered 192 / full 192 / partial 0 / outside 0`, coverage by region `{product_red: 192}` — neither the magenta (30 000 cd) nor the yellow (6 000 cd) primary is caught. **(e)** `deep_shadow` ROI **192** (`out_of_gamut_basis_points 9 411`, `below_black 0`, `minimum_linear −5 722`); whole raster **1 480 px / 256 bp** (`below_black 348`, `minimum_linear −17 776`, `maximum_desaturation 996 991`, five accompanying `delivery_range_excursion` Warnings), `technical_pass` **true**. **ROI caution:** `(2250, 4222, …)` resolves to `y 75, h 17` = 204 px, so `y_basis_points = 4223` is normative. | §2.4.3, §2.5, §4(a), §4(b), §4(d), §4(e), A15/A16/A19 |
| **P10** | Windows: do all §4 gates pass in the default lane with the MSVC FFmpeg package, and what are its numbers? | **The Windows CI job is this measurement.** If it exceeds a constant, that constant is re-baselined once, with a per-OS note — never split into a per-OS constant. | §4.1, §14 |
| **P11** | C1's proposal, the (b1) residual, the (b2) per-node delta, (a)'s skin numbers, and (g)'s starved encode | **Owed — §12 step 5 (A20).** Scenario (c) is closed by probe-3 (P3). The (b2) per-node `range_basis_points_delta` is expected **+116** by probe-1's `+22` mechanism and must be confirmed, not pinned from here; C1's proposal and the (b1) residual were measured only on the pre-amendment six-patch band; (a)'s skin `in_band` and skin-row chroma numbers likewise; (g)'s starved-encode failing direction has not been run on a CC7 source. | §4(a)(4), §4(b)(1), §4(b)(3), §4.1, §4.2 |
