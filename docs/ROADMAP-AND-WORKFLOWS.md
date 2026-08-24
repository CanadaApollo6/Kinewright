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
| Media and interchange | Import, project media, in-memory preview proxy decode/cache, hostile-media policy, save/recovery | Offline/relink workflow, explicit generated-proxy/cache management, richer metadata, interchange that preserves supported edit semantics |
| Colour | Basic exposure/temperature/tint and brightness/contrast/saturation effects, four built-in looks, agent/core `.cube` LUT support, masks, chroma key, post-compositor scope data | Colour-managed SDR correction, professional scopes, shot matching, curves/wheels, grade-scoped secondaries, human LUT workflow, look management, delivery QC |
| Audio | Multi-track mixing, buses, EQ/compression/ducking operations, waveform/transcript analysis | Manual mixer and bus UI, meters, detailed EQ/dynamics control, repair and room-tone workflows, loudness-aware delivery |
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
focused eval update. Colour and non-colour slices alternate as the primary until
CC2 is complete; dependency work may continue in the secondary lane but cannot
silently become a second unbounded project.

The first three cycle intentions are:

1. **CC0 colour contract — completed 2026-08-24.** The implementation now carries
   explicit source/working/monitoring/delivery metadata through probe, project,
   human and agent inspection, conformance, and tagged SDR Rec.709 export. The
   offline/relink and generated-proxy contract remains the next non-colour brief.
2. **Offline/relink and proxy/cache visibility** as the primary slice; begin only
   bounded CC1 groundwork after CC0's migration and metadata gates pass.
3. **CC1 managed SDR primary correction** as the primary slice; select the next
   long-form source/program usability slice from observed editing friction.

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

The present implementation is a useful base, not yet a managed colour pipeline:

- Clip effects are typed and serializable, with static values and keyframes.
- Brightness, contrast, saturation, exposure, temperature, and tint are available;
  four built-in looks and `.cube` LUT loading also exist. File-backed LUTs are
  currently a core/agent capability and intentionally lack a human file-picker
  workflow.
- Preview and export use the shared `FrameRenderer` compositor path.
- Agent scopes are measured after compositing and currently provide RGB/luma
  histograms, means, clipping counts, and a 64-column luma waveform.
- Masks and tracking exist, but the current compositor applies the mask to final
  layer alpha. That is **not** yet an effect-scoped colour secondary.
- CC0 now preserves explicit source, working, monitoring, and delivery colour
  descriptions with provenance and confidence. Probe keeps unknown values honest;
  editors and agents can inspect them and apply an undoable metadata override.
  Current export accepts only the declared 8-bit SDR Rec.709 contract and writes
  explicit H.264/YUV420P colour tags.
- Decode still converts into 8-bit RGBA and the compositor target remains
  `Rgba8Unorm`. CC0 records and validates the contract but does not perform an input
  colour transform; the defined high-precision managed transform path is CC1.
- Effect parameters are flattened into fixed compositor inputs rather than a true
  ordered colour-node stack; multiple creative LUT stages are therefore not a
  supported grading model.

Those limits determine the implementation order. More creative controls on an
implicit 8-bit path would create attractive demos but weak technical guarantees.

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
  reproducible after moving machines. Today they remain external paths cached by
  path and file metadata; project ownership is CC4 work, not current behaviour.
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

**Current status (2026-08-24): CC0 is complete.** Its exit evidence includes legacy
project migration, known/partial/unknown and 10-bit probe fixtures, visible human and
agent inspection, an undoable source override, delivery rejection outside the current
contract, and an encoded file decoded again to verify its representable Rec.709 tags.
The next primary slice is the offline/relink and proxy/cache workflow; CC1 groundwork
may now proceed only as the bounded secondary lane defined above.

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
- [Media policy](MEDIA-POLICY.md) — hostile-media behaviour and invariants.
- [Building Kinewright](BUILDING.md) — Windows, Linux, FFmpeg, and toolchain setup.
