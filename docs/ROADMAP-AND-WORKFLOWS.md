# Roadmap and development workflows

Status: active operating plan, August 2026. This is the forward-looking companion
to the numbered milestone documents. Those documents preserve what was attempted
and verified; this document defines what Kinewright works on next and how a
capability becomes part of the editor.

## The outcome

Kinewright should give an editor a strong, editable starting point quickly **and**
support the technical work required to finish it. Editorial taste is an important
advantage, but it is not a substitute for trim tools, colour correction, audio
control, compositing, media management, reliable playback, or delivery.

The programme therefore has two equally necessary product outcomes:

1. **High-leverage assistance.** A model can inspect unfamiliar footage, propose a
   credible edit, explain its choices, and save meaningful setup time.
2. **A capable editor.** A person can correct, reshape, polish, verify, and deliver
   that edit without leaving Kinewright for routine technical work.

We are pursuing workflow parity for valuable editing jobs, not copying another
editor's feature count. A narrower tool that completes an entire job reliably is
more useful than a wide collection of controls that stop at preview or cannot be
used by both the person and the agent.

> Taste checks are acceptance tests, not the roadmap.

The V10 montage machine contract/model artifact remains pending human review and is
frozen as a technical regression fixture. Its separate director reference records
accepted editorial direction; it is not a blanket acceptance of the model artifact.
We should not keep making taste-only variations of the same footage unless a new
capability or a failure on unfamiliar footage gives us a concrete reason to revisit
it.

## Who owns which decisions

| Responsibility | Product/system | Model | Editor |
| --- | --- | --- | --- |
| Exact media, time, and colour semantics | Owns and enforces | Reads them | Can inspect them |
| Valid, atomic, undoable operations | Owns and enforces | Proposes or applies them | Applies, changes, or undoes them |
| Analysis and first-pass plans | Supplies evidence and tools | Owns the proposal | Directs or rejects it |
| Creative intent and taste | Preserves choices faithfully | May offer alternatives | Owns final judgement |
| Preview/export agreement | Owns and verifies | May request proofs | Trusts what is shown |
| Delivery correctness | Owns tags, transforms, and validation | May run QC | Selects and approves output |

Models must not silently make technical assumptions that the application can
represent explicitly. They may recommend a white-balance change, a cut, or a look;
the resulting change must still be a typed operation, visible in the project,
revision-safe, undoable, and independently verifiable.

## Default investment balance

This is a planning heuristic, not a timesheet:

| Share | Workstream | Typical result |
| ---: | --- | --- |
| 50% | Capability breadth and depth | A complete editing, colour, audio, compositing, media, or delivery workflow |
| 25% | Agent integration and reliability | Better inspection, planning, proofs, revision safety, latency, and human/agent parity |
| 25% | Evaluation and evidence | Deterministic regression tests, rendered-output checks, and periodic human review |

Most evaluation effort should be objective and continuous. Human taste review is
used when the question is genuinely editorial: pacing, shot choice, emphasis,
emotional shape, or whether a creative grade supports the intended story. It is
not the primary way to test trim math, colour transforms, scope accuracy, audio
levels, serialization, preview/export parity, or platform support.

Two operating rules keep this balance honest:

- Do not run consecutive taste-only cycles on the same artifact.
- Every development cycle must advance at least one editor capability or remove a
  concrete reliability barrier; an eval by itself is not product progress.

## The capability-slice workflow

Every substantial capability follows the same path. The slice may be small, but it
must be vertical.

1. **Choose an editor job.** State the real task and the footage/project conditions
   in which it matters. Prefer “match a two-camera interview” over “add a colour
   wheel.”
2. **Write the contract.** Define inputs, outputs, units, defaults, limits, failure
   behaviour, and what the user can inspect. Separate technical correctness from
   creative preference.
3. **Add the typed core model.** Project state and mutations must serialize, migrate,
   validate, journal, branch, undo, and redo through the `Core` actor.
4. **Implement media semantics.** Decode, playback, render, proof, and export must
   agree. A proxy may be faster, but it cannot use different creative or colour
   semantics from the final render.
5. **Expose the human workflow.** The UI must let an editor perform and revise the
   job with useful feedback. A raw effect parameter is not automatically a usable
   workflow.
6. **Expose the agent workflow.** Give the model the minimum context, analysis,
   plan, action, and proof surfaces needed to do the same job. Changes remain
   revision-gated and use the generated operation vocabulary.
7. **Build deterministic evidence.** Test math, state transitions, serialization,
   rendered pixels or samples, error cases, and preview/export agreement before
   asking for a taste judgement.
8. **Exercise unfamiliar material.** Use a fixture that was not tuned during
   implementation. Include hostile or ambiguous media where the capability claims
   to handle it.
9. **Freeze the regression.** Record the accepted behaviour, remaining limits, and
   proof artifact. Update this roadmap when the next bottleneck becomes clearer.

### Definition of done

A capability is not complete until all applicable items below are true:

- The operation and project state are typed, validated, serialized, and undoable.
- Human and agent changes reach the same core operation path.
- Preview, still proof, playback, and export use compatible semantics.
- Error and unknown states are explicit; the model is not asked to guess hidden
  engine state.
- Deterministic tests cover the contract and a rendered or decoded result where
  applicable.
- Performance and memory are measured at a realistic project size.
- Windows and Linux are covered, including the supported bundled FFmpeg path;
  hands-on validation uses Windows and Omarchy rather than assuming Ubuntu CI alone
  represents the supported desktop environments.
- The user-facing workflow is documented, and known deferrals are stated without
  implying support that does not exist.
- A creative workflow has a before/after proof and human acceptance; a purely
  technical workflow has an objective exit gate.

## Capability portfolio

The order within each track is driven by complete user jobs and dependencies.
Colour begins immediately, while non-colour work continues in parallel.

| Track | Existing base | Next workflow goals |
| --- | --- | --- |
| Editorial and long-form | Three-point edits, slip/roll/slide, replace, fit-to-fill, bins, string-outs, sync groups, transcript editing | Dual source/program workflow, source patching and track targeting, compound/nested structure, long-sequence navigation and revision |
| Media and interchange | Import, project media, verified source identity, offline/changed status, undoable relink, ephemeral scaled preview memory, scoped cache visibility/clearing, hostile-media policy, save/recovery | Generated playable proxies, richer metadata, managed/project-relative media, interchange that preserves supported edit semantics |
| Colour | Managed SDR Rec.709 input → high-precision working → primary correction → monitor/delivery pipeline, typed source assumptions and metadata, ten primary controls, CPU/GPU/proof/export parity, four built-in looks, agent/core `.cube` LUT support, masks, chroma key, professional post-composite scopes, ROI/temporal evidence, and reference-shot matching proposals | Curves/wheels, grade-scoped secondaries, human LUT workflow, look management, delivery QC |
| Audio | Multi-track mixing, buses, EQ/compression/ducking operations, waveform/transcript analysis, typed loudness/true-peak/range delivery QC on every verified export (AD0) | Loudness normalization as an operation, a true-peak limiter, manual mixer and bus UI, meters, detailed EQ/dynamics control, repair and room-tone workflows |
| Motion, compositing, and retiming | GPU compositor, effects, keyframes, masks/tracking, transitions, constant-speed controls | Keyframe editing UI, speed ramps, effect-scoped mattes, adjustment/compound layers, transform and compositing polish |
| Multicam | Sync groups and agent speaker/angle planning primitives | Angle viewer, live switching and revision, audio-follow policy, explicit master-audio handling |
| Delivery and performance | Shared render path, H.264/AAC export queue and profiles | Codec/preset breadth, colour/audio tags and QC, cache control, long-project responsiveness, interruption and recovery testing |
| Creator workflows | Captions, titles, transcript operations, reframing primitives | Caption finishing, reusable packages/templates, aspect-ratio variants, reviewable batch versioning |

This table is intentionally broader than the current eval programme. A general
video editor will not reach practical parity by optimizing montage taste alone.

## Near-term sequence

Planning runs in target three-to-four-week cycles. The exit gate, not the calendar,
decides whether a slice is complete. Each cycle names one primary capability slice
and owner in its implementation brief, one bounded reliability improvement, and one
focused eval update. With CC2 complete, colour and non-colour slices continue to
alternate as the primary; dependency work may continue in the secondary lane but
cannot silently become a second unbounded project.

The first three cycle intentions are:

1. **CC0 colour contract — completed 2026-08-24.** The implementation now carries
   explicit source/working/monitoring/delivery metadata through probe, project,
   human and agent inspection, conformance, and tagged SDR Rec.709 export.
2. **M41 offline/relink and cache visibility — completed 2026-08-24.** Projects
   persist a verified source identity, report live availability, relink through one
   revision-gated undoable operation, and expose honest scoped cache inspection and
   clearing to both human and agent workflows. Ephemeral scaled preview memory is
   explicitly distinguished from generated playable proxies, which remain deferred.
3. **CC1 managed SDR primary correction — completed 2026-08-24.** Supported SDR
   sources now enter an explicit high-precision working pipeline, retain recoverable
   over-range values through primary correction, and share compatible CPU, GPU,
   monitor-proof, and tagged H.264 delivery semantics. Editors and agents use the
   same ten typed primary controls and receive explicit errors for unsupported or
   ambiguous source profiles.

4. **M42 source/program patching and track targeting — completed 2026-08-24.**
   Source and Program are independently addressable, every video/audio route is
   visible, and one revision-safe compound operation performs single- or dual-route
   Insert and Overwrite without double-rippling. Human and agent edits fail closed
   unless the referenced source has just been verified. The objective contract and
   deferrals are recorded in `M42-SOURCE-PROGRAM-PATCHING.md`.

5. **CC2 scopes and matching — completed 2026-08-24.** The shared deterministic
   engine now supplies bounded waveform, RGB parade, vectorscope, histogram,
   clipping, ROI, temporal, and signed comparison evidence. The editor captures a
   full-raster reference asynchronously, while agent tools inspect shots and return
   exact revision-gated primary-correction operations without applying them. The
   implementation contract and remaining hands-on platform smoke gate are recorded
   in `CC2-SCOPES-AND-MATCHING.md`.

6. **CC3 curves and wheels — implemented 2026-08-24, pending platform smoke.**
   Two ordered managed colour nodes, `color_wheels` (ASC CDL-style slope/offset/
   power with per-channel and master integer controls) and `color_curves`
   (master/red/green/blue monotone cubic Hermite curves with integer points),
   execute in `clip.effects` order inside the CC1 working pipeline through an
   invertible `grade709` grading encoding. Both nodes are serialized as ordinary
   integer parameters, carry a `bypass` token, keyframe under an explicit policy,
   reset through existing operations, and are inspected, planned
   (`plan_color_wheels`/`plan_color_curves`), and rendered on the GPU node stack
   with an independent CPU reference and a fixture suite in `cc3_fixtures.rs`.
   The contract is `CC3-CURVES-AND-WHEELS.md`.

7. **CC4 look management — implemented 2026-08-25, pending platform smoke.**
   LUT looks are project-owned, content-hashed assets in a project-relative
   sidecar store (`<stem>.kinewright-assets/luts/<sha256>.cube`) with typed
   availability and explicit restore/replace recovery; two ordered managed
   nodes, `technical_lut` (input transform stage) and `creative_look` (look
   stage), execute on the CC3 node stack with normative tetrahedral
   interpolation, an additive out-of-domain rule, linear-light mix, and
   bypass; a stage-ordering rule is enforced by Core rejection; the four
   legacy built-in looks are deterministic, hash-pinned generated assets; the
   GPU binds one `Rgba32Float` LUT atlas; the app gains the human `.cube`
   import, a look browser, mix/bypass/A-B, and a dialog-free `write_project`
   that carries the store on Save As; the agent gains `list_look_assets`,
   confirmation-gated `import_lut_asset`, `plan_technical_lut`,
   `plan_creative_look`, and look proofs. The contract is
   `CC4-LOOK-MANAGEMENT.md`.

8. **CC5 secondaries — implemented 2026-08-25, pending platform smoke.** A
   matte belongs to its correction node: `primary_correction`, `color_wheels`,
   `color_curves`, and `creative_look` carry an optional matte of up to four
   aspect-corrected rotatable windows (rect/ellipse, symmetric feather,
   per-window invert, union/intersection) and an HSL qualifier evaluated on
   the node input in `grade709`, applied as `out = x + (node(x) − x)·m` with a
   per-pixel exact identity at `m = 0`; alpha is never touched and the layer
   `mask` effect is unchanged. The GPU node stack carries a 64-word matte
   block per node and a matte-debug selector that renders coverage; an
   independent CPU reference evaluates the same contract at pixel centres;
   `matte_proof_for_document`, `inspect_grade_matte`, matte-scoped scopes
   through the unchanged CC2 engine, `track_matte_window` on the existing
   tracker with M40 smoothing, and `plan_secondary_correction` complete the
   agent surface; the inspector gains a matte section, a preview overlay with
   drag editing, and a matte view. The contract is `CC5-SECONDARIES.md`.

9. **CC6 QC and managed delivery — implemented 2026-08-25, pending platform
   smoke.** A named high-precision stage, `working_linear_post_composite`, and
   `Analysis::working_proof_for_document` read the production `Rgba16Float`
   composite back as linear f32 before any encode; a pure core QC engine
   (`color_qc`) measures range (delivery-encoded clamp events per channel),
   gamut (negative linear channels with the desaturation fraction), a forward
   BT.709 limited-range Y′CbCr reference at 8 and 10 bits, region-scoped skin
   diagnostics against a band derived from the CC5 skin patches, a two-mode
   typed delivery tag check, and per-node clipping attribution by effect
   removal — all integer-reported and evidence-only. Managed delivery widens
   by exactly one lane (10-bit H.264, `yuv420p10le`) with typed
   `DeliveryColorError` rejections, the delivery intermediate is quantized on
   swscale's 16-bit RGB white (`65280`), and `Analysis::verify_delivery_output`
   decodes a written file at sampled frames, compares the native luma plane
   and RGB against the full-resolution delivery reference under named
   per-lane budgets, measures decoded Y′CbCr legality (EBU R 103), and probes
   tags — wired into the export queue, `get_export_jobs`, and the export
   dialog. The agent gains `get_color_qc`; the app gains a Colour QC window,
   a QC clipping mask view, absolute clipping in the scopes panel, a per-node
   clipping line, and the 8/10-bit choice with a post-export verification
   block. The exit gate is a synthetic 60-frame source exported at both
   depths on both CI operating systems and gated on tag, range, and
   visual-difference budgets. The contract is `CC6-QC-AND-MANAGED-DELIVERY.md`.

Before CC3 started, a six-lane review of CC1 and CC2 (2026-08-24) fixed the
defects recorded in `CHANGELOG.md` and hardened both fixture suites so that
every control has an analytic expected value and no parity case is vacuous.
10. **CC7 workflow evaluation — implemented 2026-08-27, pending the real-harness
   run and the blind review.** No colour feature and no MCP tool; CC7 consumes the
   CC0–CC6 surface and records the margins. A core scenario authority
   (`cc7_scenarios`) pins six synthetic scenarios with analytic expectations —
   mixed-camera interview, wrong white balance and underexposure (recoverable
   and beyond control authority), a BT.709-tagged log-like carrier undone by an
   imported inverse `.cube`, qualifier-only product/skin containment plus a
   window-only feather-edge case, the built-in `warm` look with an exact
   out-of-gamut count, and a tracked secondary with a one-sample occlusion — and
   test-support media generators (`cc7_sources`, compiled for tests and the
   `test-util` feature) author every raster in Rust and mux
   it lossless. Every objective claim is an ordinary `cargo test` on both CI
   operating systems: media gates on the canonical document of each scenario
   (neutral-patch spread, luma delta, skin band, matte coverage, gamut, track
   confidence and containment, decoded delivery at both depths under CC6's
   unchanged budgets), scripted `cc7_` MCP tests that complete every workflow
   through the real endpoint and commit the canonical document, and app tests
   that prove the person path expresses the same operations through the
   inspector builders. The model path is a sixth eval suite,
   `color-workflow-v6`, whose colour measurements are computed inside the
   runner and carried on the outcome; its review package is blinded to machine
   provenance (`blind/` artefacts and a review form keyed by a derived id, the
   key outside), and `human-review.json` moves to schema 2 with per-task
   questions. The exit gate is the technical gates green on both CI operating
   systems with the human reviewer left only the matrix's creative questions.
   The contract is `CC7-WORKFLOW-EVALUATION.md`.

With CC7 the colour programme table is complete; HDR, camera RAW, ACES/OCIO,
calibrated-monitor output, and temporal noise reduction remain deliberate later
programmes, and the M40 gauntlet continues to rotate colour tasks as
regressions.

11. **Post-CC7 review and hardening — 2026-09-02.** `ROADMAP-REVIEW-2026-09.md`
   records the review. The hardening it asked for landed in one cycle: the
   media crate's test generators no longer ship in the desktop binary (the
   agent crate's `eval` feature owns them), `server.rs` is split into
   tool-family submodules, the six colour fixture files share one support
   module, CI caches the toolchain and the pinned FFmpeg build, runs the media
   fixture suite in its own job, and has a timeout. `COLOR-SMOKE-TEST.md` is
   the hands-on procedure for the CC3–CC7 platform gates, written for a
   person who is not a colourist against known-answer footage that
   `kinewright-eval --write-color-smoke-media` materializes.

12. **AD0 audio delivery contract — foundation implemented 2026-09-02,
   pending platform smoke.** The first non-colour primary slice, chosen by
   the bottleneck M40 observed in a real edit (an event cut delivered at
   −39.9 LUFS). Typed `AudioDeliveryTarget` presets (measure-only, streaming
   −14, podcast −16, EBU R 128, ATSC A/85) are a job parameter beside the
   delivery depth; every verified export now decodes its own audio, measures
   BS.1770 integrated loudness, 4× oversampled true peak, and EBU Tech 3342
   loudness range, and reports typed exceptions with the gain that would
   reach the target. The export dialog gains the preset choice, an
   `AUDIO OUT OF SPEC` status, and a `DECODED AUDIO` block. Nothing applies
   gain. The contract, the fixtures, and the deferrals (normalization as an
   operation, a true-peak limiter, meters and the mixer, the agent tools) are
   in `AD0-AUDIO-DELIVERY.md`.

Within that cadence, three workstreams remain active:

1. **Colour foundation and correction.** Implement the staged workflow below,
   beginning with explicit colour metadata and a managed SDR path.
2. **Non-colour breadth.** On its primary cycles, start with media relink/proxy
   control and long-form source/program usability, then deepen manual audio and
   motion/retiming workflows according to the bottleneck observed in real edits.
3. **Rotating evaluation.** Keep the M40 generalization gauntlet, but rotate across
   interview, event/multicam, montage, dialogue, product, and colour tasks. Reuse
   old candidates as regressions rather than the main creative target.

The workstreams meet at shared infrastructure—typed operations, revision-gated
plans, proofs, playback/render agreement, and cross-platform delivery—but they do
not wait for another taste benchmark before progressing.

## Colour correction programme

### Product boundary

Kinewright distinguishes three jobs:

1. **Technical normalization** interprets the source correctly and maps it into the
   project working space.
2. **Correction and matching** makes shots readable, neutral where intended,
   consistent with one another, and safe for the target delivery.
3. **Creative grading** deliberately shapes palette, contrast, focus, and mood.

The application owns the first job. The model may propose the second with measured
evidence. The editor owns the intent and final acceptance of the third. “Auto
grade” must never collapse all three into an unexplained transform.

### Current foundation and limits

The present implementation includes the first managed SDR vertical slice, with
deliberate limits that define the remaining colour work:

- Clip effects are typed and serializable, with static values and keyframes.
- The managed `primary_correction` node is the only current-generation colour
  control. The older display-coded `brightness`, `contrast`, and `saturation`
  effects load for compatibility only, are not offered for new insertion, and
  report `legacy_colour_semantics`; `color_grade` is canonicalized to
  `primary_correction` on load. Four built-in looks and `.cube` LUT loading also
  exist as post-primary compatibility stages. File-backed LUTs are currently a
  core/agent capability and intentionally lack a human file-picker workflow.
- Managed preview, isolated full-resolution proof, and export share the production
  visual-layer resolution and compositor semantics.
- CC2 scopes are measured at the named managed post-composite monitoring stage and
  provide bounded full-raster or explicitly labelled proxy histograms, statistics,
  clipping, waveform, RGB parade, vectorscope, geometric ROI, temporal sampling,
  and signed reference comparison. The same typed engine feeds the non-blocking
  editor panel and the read-only agent analysis/matching tools.
- The layer `mask` effect applies to final layer alpha and remains a
  compositing operation. Since CC5, effect-scoped colour secondaries are the
  node-owned mattes on the managed colour nodes, which never touch alpha.
- CC0 preserves explicit source, working, monitoring, and delivery colour
  descriptions with provenance and confidence. Probe keeps unknown values honest;
  editors and agents can inspect them and apply an undoable metadata override.
  Delivery conformance accepts only an explicitly supported SDR Rec.709 contract
  and writes explicit H.264/YUV420P colour tags.
- CC1 decodes supported integer SDR sources through an explicit managed conversion
  into `Rgba16Float`, applies the canonical primary pipeline without intermediate
  display-range clamping, and performs the monitoring/output transform only at the
  named boundary. Unsupported and unresolved profiles fail with typed recovery
  information rather than silently falling back to an implicit transform.
- The managed cache accounts for high-precision working bytes and returns an
  oversized current frame without retaining it beyond the configured bound.
- The primary node supplies exposure, temperature, tint, contrast, contrast
  pivot, blacks, shadows, highlights, whites, and saturation with stable defaults,
  limits, serialization, undo/redo, editor controls, agent planning, and proof
  manifests. There is no hue or midtone control yet; those belong to CC3.
- `primary_correction` nodes execute in serialized `clip.effects` order as an
  ordered node stack. Only the legacy display-coded effects, built-in looks, and
  `.cube` LUTs are still flattened into fixed compositor inputs; multiple creative
  LUT stages are therefore not a supported grading model until CC4.

Those limits determine the remaining implementation order. With CC2 scopes and
shot matching complete, CC3 expands the correction model with curves and wheels.

### Colour architecture principles

- Start with a correct, well-tested SDR Rec.709 workflow. Keep unknown metadata
  explicit. Do not silently label an unknown source Rec.709.
- Preserve source, project/working, monitoring, and delivery descriptions as
  distinct concepts, even when their values are initially the same.
- Use a high-precision intermediate before serious matching, curves, compositing,
  or HDR work. Clamping to display range must not occur between correction stages.
- Keep input transforms, corrections, creative looks, and output transforms
  separately inspectable and ordered.
- Compute scopes from a named stage of the managed pipeline and report which stage,
  resolution, range, and region they describe.
- Make a secondary matte belong to its correction node. Alpha masking a whole layer
  is a different compositing operation.
- Store imported LUTs as project-owned, content-hashed assets so a project remains
  reproducible after moving machines. Since CC4 they live in the project's
  `.kinewright-assets` sidecar store keyed by SHA-256; only the legacy
  `cube_lut` compatibility stage still resolves an external path.
- Use the same transform definitions on Windows and Linux. Avoid results that depend
  on undocumented FFmpeg/swscale defaults or one GPU backend.
- Agent proposals include confidence, assumptions, before/after proofs, and scope
  deltas. No silent automatic correction.

### Editor and agent workflows

#### 1. Inspect and classify

The editor selects a clip, group, or sequence and sees the interpreted source
description, working space, monitoring transform, delivery target, scopes, and any
unknown or conflicting metadata. The agent receives the same context and can flag
likely range, transfer, white-balance, clipping, or consistency problems without
changing the project.

Exit evidence: metadata provenance is visible; scopes identify their measurement
stage; unknown input produces a warning and an explicit override workflow.

#### 2. Make a primary correction

The editor adjusts exposure, white balance, contrast/pivot, tonal balance, and
saturation using controls and scopes. The agent may analyze a shot and propose a
revision-gated primary correction with a neutral reference and measured deltas.

Exit evidence: neutral charts and ramps behave within tolerance; controls are
stable at boundaries; before/after frames and scopes agree with the full render.

#### 3. Match a scene

The editor chooses a hero/reference shot and applies a starting match to a group,
then trims individual clips. The agent can identify outliers and propose per-shot
changes while retaining the reference and group relationship.

Exit evidence: a mixed-camera sequence has reduced objective exposure/white-balance
variance without flattening intentional differences; each adjustment remains
separately editable and undoable.

#### 4. Apply a creative look

Technical normalization and correction remain upstream. The editor auditions a
built-in or imported look at adjustable strength, compares it with bypass, and can
apply it to a clip or managed group. The agent may offer named alternatives and
describe their intended effect, but human review decides whether the look works.

Exit evidence: look assets are portable and hashed; ordering is explicit; bypass
is lossless; no display/output transform is mistaken for a creative LUT.

#### 5. Make a secondary correction

The editor creates an HSL qualifier or geometric window for a specific correction,
inspects the matte, refines/feathers it, and tracks or keyframes it through the
shot. The agent may propose the selection and tracking plan, with a manual fallback
when confidence is low.

Exit evidence: matte inspection matches the pixels affected by that correction;
tracking is revision-gated; skin/product fixtures show no unintended alpha change
or contamination outside the matte.

#### 6. QC and deliver

The editor checks gamut/legal limits, clipping, shot consistency, skin-tone
diagnostics where relevant, and the delivery transform. The agent can summarize
remaining exceptions and render full-resolution proof frames or a review segment.
The exported file is decoded again and compared with the managed render.

Exit evidence: output tags and transforms are explicit; decoded delivery is within
tolerance; warnings distinguish intentional creative excursions from accidental
technical failures.

### Staged implementation

Stages are dependency-ordered; they need not become new numbered product milestones.
CC0 explicitly covers probe/import, serialized project migration and fixture
constructors, compositor inputs, `ExportSettings`, and FFmpeg stream/container
metadata. Later stages cannot treat those surfaces as implicit or start before the
required earlier exit gate passes.

**Current status (2026-09-02): CC0, M41, CC1, M42, CC2, CC3, CC4, CC5, CC6, and CC7
are complete apart from the CC3–CC7 hands-on platform smoke gates
(`COLOR-SMOKE-TEST.md` is the procedure) and CC7's real-harness eval run and
blind review. The AD0 audio delivery foundation is implemented and awaits the
same smoke.** CC0's exit evidence
includes legacy project migration, known/partial/unknown and 10-bit probe fixtures,
visible human and agent inspection, an undoable source override, delivery rejection
outside the current contract, and an encoded file decoded again to verify its
representable Rec.709 tags. M41 then completed verified source identity, offline and
changed-media diagnosis, deterministic relink, and cache visibility. CC1 adds the
explicit high-precision managed SDR input/correction/output path, ten typed primary
controls, full-raster proofs with source/provenance manifests, verified-source export
preflight, objective ramp/chart/control/cache/delivery fixtures, and CPU/GPU parity
evidence. M42 adds independent Source/Program monitoring, explicit video/audio
patch destinations, atomic compound edits, and mandatory live source revalidation.
CC2 adds deterministic full-raster-aware scopes, ROI and temporal evidence, an
asynchronous human reference workflow, and revision-gated two-shot matching
proposals that expose exact operations without hidden changes. CC3 adds the
ordered wheels and curves nodes, the `grade709` grading encoding, the widened
GPU node-stack ABI, the wheel and curve-editor widgets, and evidence-only
wheel/curve planners. CC4 adds project-owned hashed LUT assets with a
relocatable sidecar store, the ordered technical/creative LUT nodes, the
built-in looks as generated assets, the human `.cube` workflow, and look
planners. CC5 adds node-owned mattes (windows, HSL qualifier, feather,
keyframes, tracking), matte inspection, and matte-scoped scopes with an
affected-pixel containment gate. CC6 adds the linear working-stage proof, the
colour QC engine (range, gamut, Y′CbCr legality, skin diagnostics, tag checks,
per-node attribution), the 10-bit H.264 delivery lane with typed rejection, and
decoded-output verification of every export. CC7 adds the scenario authority,
the lossless synthetic scenario sources, the per-scenario technical gates on
both CI operating systems, the scripted agent and person paths, the
`color-workflow-v6` suite, and the blinded review package; with it the colour
programme table below is complete.

| Stage | Deliverable | Exit gate |
| --- | --- | --- |
| CC0 — Colour contract | Typed `ColorDescription` for source, project, monitor, and delivery: primaries, transfer, matrix, range, white point, bit depth, confidence, and provenance | Round-trip/migration tests; known and unknown files inspect correctly; explicit override is undoable |
| CC1 — Managed SDR primary | Defined input → high-precision working → correction → output path; complete primary controls with stable units and defaults | Reference chart/ramp pixel tests; preview/proof/export parity on CPU/GPU/platform fixtures |
| CC2 — Scopes and matching | Full-resolution-aware waveform, RGB parade, vectorscope, geometric ROI scope, temporal sampling, reference-shot comparison | Analytic fixture tests plus a two-camera match with recorded deltas and no hidden auto-change |
| CC3 — Curves and wheels | Lift/gamma/gain or shadows/midtones/highlights, RGB/luma curves, channel controls, reset/bypass and keyframing policy | Identity/monotonicity/boundary tests; serialized, undoable human and agent operations |
| CC4 — Look management | Ordered technical transforms versus creative LUTs, adjustable mix, project-owned hashed LUT assets, later 1D shaper support | Relocatable project proof; consistent interpolation and ordering; missing asset has explicit recovery |
| CC5 — Secondaries | Grade-node matte architecture, HSL qualifier, windows, feathering, tracking/keyframes, matte inspection and matte-scoped scopes | Affected-pixel tests and tracked-shot proof; no misuse of final layer alpha |
| CC6 — QC and managed delivery | Gamut/legal checks, skin diagnostics, colour tags/transforms, high-quality full-resolution path, decoded-output comparison | Cross-platform encoded fixture passes tag, range, and visual-difference budgets |
| CC7 — Workflow evaluation | Mixed-camera interview, poor white balance/exposure, skin and product, log-like input, creative look, and tracked secondary | Technical gates pass independently; blind human review is limited to creative and workflow-quality questions |

HDR, camera RAW controls, ACES/OCIO integration, calibrated-monitor output, and
advanced temporal noise reduction are deliberate later programmes. CC0–CC6 should
leave room for them, but we should not claim them before the SDR path is explicit
and high precision.

### Agent surface direction

Exact tool names are fixed in each capability contract, but the intended separation
is:

- `get_color_context`: source/project/monitor/delivery descriptions and provenance.
- `get_video_scopes_v2`: named pipeline stage, region, temporal range, sampling
  resolution, waveform/parade/vectorscope/histogram data, and clipping/gamut data.
- `analyze_color_shot`: evidence-only diagnosis with confidence and assumptions.
- `plan_primary_correction`: a revision-gated, inspectable primary proposal.
- `plan_shot_match`: reference/group-aware per-shot proposal, not a flattened LUT.
- `inspect_grade_matte`: matte frame/statistics for a specific secondary node.
- `get_color_qc`: remaining range, gamut, tag, match, and delivery exceptions.
- `render_color_proof`: before/after or bypass proof from the full managed path.

Analysis tools do not mutate. Plan tools return the exact operations they intend to
apply and require the project revision they analyzed. Proof tools identify the
render stage and cannot substitute a proxy-only result for delivery verification.

### Colour evaluation matrix

| Fixture | Objective checks | Human question, if any |
| --- | --- | --- |
| Synthetic ramps, charts, range and transfer cases | Transform math, neutrality, clipping, bit-depth tolerance, scope accuracy | None |
| Mixed-camera interview | Per-shot balance deltas, reference retention, skin region statistics, render parity | Does the match preserve natural and intentional differences? |
| Wrong white balance / underexposure | Recovery behaviour, noise/clipping warnings, control limits | Is the proposed compromise acceptable? |
| Product and skin shots | Qualifier containment, hue stability, matte edges | Does attention remain on the intended subject? |
| Creative day/night or stylized look | Ordering, asset portability, bypass, gamut warnings | Does the look support the story? |
| Moving tracked secondary | Track confidence, matte containment over time, fallback behaviour | Are any visible corrections distracting? |
| Encoded delivery | Tags, decoded pixel/sample comparison, platform consistency | Only if a codec limitation creates a visible trade-off |

No single hero shot can establish colour capability. The suite must include diverse
lighting, skin tones, camera encodings, saturation, motion, and deliberate creative
exceptions.

Before implementation, every colour slice brief pins numeric thresholds for its
fixtures. At minimum, the evidence records first/median/99th luma percentiles,
neutral-patch channel spread (and Delta E 2000 once the working-space conversion is
defined), clipping in basis points, affected-pixel containment for mattes, and
max/P99/mean absolute channel differences for preview/proof/export comparisons.
Identity transforms target no more than one 8-bit output code value of difference;
lossy encoded-delivery tolerances are codec-specific and must be baselined rather
than reused as compositor tolerances. A phrase such as “within tolerance” is not an
exit gate until that slice's brief supplies the number and sampling method.

### Windows and Omarchy release evidence

Windows and Omarchy are the active hands-on systems. Ubuntu CI remains useful
generic Linux coverage, but it is not evidence that the Omarchy desktop, GPU,
audio, or packaging path works. Until an Arch-family CI job covers the same surface,
a release that changes native media, GPU, audio, or packaging behaviour records a
manual Omarchy smoke test using the repository's `pacman` dependency path and
bundled FFmpeg setup. The record includes the Omarchy snapshot, kernel/GPU driver,
FFmpeg build hash, Rust version, and results for workspace build/test, application
launch, media import/playback, and a fixture render/export. Windows retains its CI
coverage and receives the equivalent hands-on smoke test for release-affecting
changes. An Arch-family automated build/test job should eventually replace the
manual build portion when it can reproduce the supported runtime accurately.

## Programme scorecard

The roadmap is healthy when all of these improve, not merely the taste score of one
montage:

- Time from import to a credible first watchable cut.
- Time and number of interventions from first cut to accepted delivery.
- Number of valuable workflows completed end to end by both person and agent.
- Percentage of core operations with usable human and agent surfaces.
- Preview/proof/export parity and decoded-delivery error rates.
- Undo, revision-conflict, recovery, and portability failure rates.
- Playback, scope, and render performance on long realistic projects.
- Objective eval coverage versus subjective review time.
- Generalization across footage, edit type, platform, and delivery target.

Review this plan after each completed vertical slice. Update the portfolio and
near-term sequence based on observed editor friction, while keeping the outcome,
ownership boundary, and definition of done stable.

## Related documents

- [Model-first editor](MODEL-FIRST-EDITOR.md) — architectural product thesis.
- [Current product position](PRODUCT-POSITION-M35-2026-08.md) — positioning and
  competitive boundary at M35.
- [M32 editorial credibility verification](M32-EDITORIAL-CREDIBILITY-VERIFICATION.md)
  — existing editorial mechanics and source-monitor baseline.
- [M33 parametric depth verification](M33-PARAMETRIC-DEPTH-VERIFICATION.md) —
  existing colour, audio, automation, and retiming primitives.
- [M40 generalization gauntlet](M40-GENERALIZATION-GAUNTLET.md) — current
  real-footage evaluation programme.
- [M41 offline/relink and cache visibility](M41-OFFLINE-RELINK-CACHE-VISIBILITY.md)
  — verified source identity, deterministic relink, and owned-cache contract.
- [M42 source/program patching and track targeting](M42-SOURCE-PROGRAM-PATCHING.md)
  — independent Source/Program workflow, explicit routes, and verified compound
  edits.
- [Roadmap review, September 2026](ROADMAP-REVIEW-2026-09.md) — the post-CC7
  review, the hardening list, and the case for audio delivery next.
- [Color pipeline smoke test](COLOR-SMOKE-TEST.md) — the hands-on procedure
  for the CC0–CC7 platform gates.
- [AD0 audio delivery](AD0-AUDIO-DELIVERY.md) — the audio delivery contract,
  decoded-file measurement, and QC engine.
- [Media policy](MEDIA-POLICY.md) — hostile-media behaviour and invariants.
- [Building Kinewright](BUILDING.md) — Windows, Linux, FFmpeg, and toolchain setup.
