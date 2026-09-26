# MO2 — Blend, adjustment, transitions, solids, solo

> Status: accepted design (revision 2 + promotion edits P1–P13).
> Recipe: v2. Binding: `MO0` §§4/7/8/10/11, `MO1`, roadmap motion § (Production-driven order 2026-09-25).
> Style: ≤ ~720 lines; symbols not `file:line`; numbered testable rules each naming owning stage.

## 0.1 Promotion edits

| Edit | Section |
|---|---|
| P1 | §3 αs definition; §5 R18 strength |
| P2 | §3 R10; §4 R14/R16 |
| P3 | §6 R20/R21; §13 gates 5–6 |
| P4 | §4 R13; §6 R23; §13 gate 5; §14 |
| P5 | §4 R12/R15/R16; §9 R27; §13 pins |
| P6 | §3 R11; §2 R3 pins |
| P7 | §2 R8 |
| P8 | §2 R7; §13 gate 11 |
| P9 | §7 R24/R25 |
| P10 | §9 R28; §13 gate 10 |
| P11 | §7 R25; §11 R30; §12; §13 pins |
| P12 | §11 R32 |
| P13 | §8 R26 |

## 0.2 Implementation errata (Part A)

- ME1 → §12 Part A scope: the agent read-back (`render_timeline_state`,
  clip info) names the new kinds (`adjustment`, `solid=#rrggbb`) and a
  non-`normal` blend (` blend=<mode>`) in Part A, not Part B. Evidence:
  otherwise the four R8 mutators' results read back as `asset=0
  <missing>`, a false missing-media claim. `Normal` output stays
  byte-identical (existing goldens unchanged); +83 production lines.
- ME2 → §4 R13 (Part B1): non-`Normal` adjustment Push costs **2** copies,
  not 3. `D0` is snapshotted into A before the backdrop draw and nothing
  overwrites A afterwards; the shifted backdrop is re-snapshotted into a
  second pooled texture B, so the adjustment samples `D0` from A and blends
  against B. The "preserve `D0` before overwriting the destination
  snapshot" copy only exists if the re-snapshot reuses A. Semantics are
  unchanged (gate 5's adjustment-Push cases, GPU ≡ twin); the copy-count
  probe pins 0/0/0/1/1/1/1/2/2 across Normal, Slide, Wipe, blend,
  adjustment, Normal Push, Normal adjustment Push, blend Push and blend
  adjustment Push. Ledger two pooled snapshots, not three.
- ME3 → §4 R16 (Part B1): the MO1 R26 re-gate runs each golden's raster,
  size and transform through `WorkingFrame::from_display_frame`, the input
  type every render composites, not the goldens' 8-bit `FrameTexture`.
  Evidence: lavapipe filters `Rgba8Unorm` at 8-bit precision (up to 0.008
  linear off an exact bilinear on the high-frequency gradient), which no
  twin reproduces portably. That path is fixture-only (production never
  composites 8-bit textures). With f16 inputs, GPU ≡ twin within 5e-4 on
  all 13 cases.
- ME4 → §3 R10 (B1 fix round 1, review-1 B1 / review-2 B2 + S1): R10's
  checked set is every *special* layer — non-`Normal` blends **and every
  adjustment, `Normal` included** (selector word 8: a `Normal` adjustment
  validated against its snapshot; under Push its covered pixels' backdrop
  is the unshifted `D0(x)` by R21, so no extra copy). The source, the
  below value, the blend result and alpha must be finite, tested on the
  bits before `min`/`max` can erase them; magnitude is checked only on the
  value the target stores (`α·B + (1−α)·D`), so `Add(40000,40000)` at
  α=0.25 stores 49,984 instead of refusing. `Normal` pixel layers stay
  unchecked (the CC3 overflow contract and R12's untouched fast path).
- ME5 → §4 R14 / §6 R21 (B1 fix round 1, review-2 B1 + S2): no NDC
  varying. The GPU decides coverage on the exact fragment position `i + ½`
  against an edge the host moves into output pixels, rounded up to the
  next pixel centre (`⌈e·n − ½⌉ + ½`), and the twin compares `(i + ½) <
  e·n` in f64. Keep-`<`-edge therefore holds exactly when the centre
  fraction is `< e`. Evidence: the interpolated NDC misplaced tie centres
  on odd rasters (17×11 at `p = ½`: up to 195 bad channels per direction).
  Separately, a centre that is rasterized on a quad's top/left edge
  interpolates uv a few ulps below 0, and the crop test zeroed it (the
  isolated down Push/Slide mismatch). The crop now tests uv clamped to
  [0, 1]. Pinned by the odd/even transition grid and the exact-centre
  probe (M22/M25 and both fixes' reversals killed).

## Changes in revision 2

| Finding | Change → section |
|---|---|
| B1 | Extended-domain Screen/Overlay + numeric vectors → §3 R9 |
| B2 | Opaque accumulator, emit `(B,αs)` (verified vs `Compositor::new` + `fragment_main`) → §3 R9b |
| B3 | Single-snapshot push with re-snapshot, OOB→black → §4 R13, §6 R21/R23 |
| B4 | Adjustment honours its blend; support table with refusals → §5 R18 |
| B5 | First-lander format owner, union v2, no-tag rule (N6 R-B) → §2 R7, §11 R30 |
| B6 | Solids via `WorkingFrame::from_display_frame` (N6 R-A); MO2 non-finite refusal (N6 R-C) → §2 R3, §3 R10 |
| B7 | Absolute floors, live+idle accounting, failing controls → §9 R28, §13 gate 10 |
| B8 | Explicit raster/sampling twin, per-domain tolerances, gate 1 → pins → §4 R14–R16, §9 R27, §13 |
| S1 | Output-space coverage, distinct endpoints, signed vectors, odd durations → §6 R20–R21 |
| S2 | Solo fallback (N6 R-F); bundle→MO6 (N6 R-D); clip-local adjustment keys; `portrait_feed`→DL1 (N6 R-E) → §7, §8, §14–§15 |
| S3 | Sampling/transport contract, serialized-byte pin, provenance → §7 R24–R25 |
| S4 | Op signatures + validation matrix; typed variants; `Track` subjects; solo error enum → §2 R8, §10 R29 |
| S5 | Both-commit re-pins; 153/60/93 → 157/64/93 → 158/64/94 → §7 R25, §12 |
| S6 | Exact ABI assert, per-backend baselines, blit rule, instrumentation → §4 R14–R15 |
| S7 | N6 R-A order; A+B land together; Part-B decomposition; resolved-layer seam → §11–§12 |
| Nits | MO1-R22-qualified gesture ref; creation defaults; staged trace; behavioural space tests → §8 R26, §3 R11, §R |

## 0 Goal and scope

**Goal.** Give every clip a blend mode, ranged adjustment looks, straight
push/slide/wipe transitions, and solid-colour clips — by hand and by agent —
through one sub-composite pass whose `Normal` path is bit-identical to pre-MO2.

In scope: `BlendMode` on `Clip` (`Normal` + six, scene-linear); the
sub-composite pass with a pooled `Rgba16Float` accumulator; `ClipContent::`
`Adjustment`/`::Solid`; 12 push/slide/wipe descriptors; `preview_solo`; the
CPU twin (also re-gating MO1 transform goldens, MO1 §11); fps floor, VRAM
ceiling, format-version predicate. GUI: blend picker, solid/adjustment
authoring, transition menus, solo strip, overlay transform drag. Agent:
generated mutators + `preview_solo`, registry-only, served quad unchanged.

Out of scope: MO3 compounds (but `preview_solo` accepts any clip id, so nests
ride free later); MO4 retime; MO5 multi-window/track mattes (mask stays the
single-shape arm); MO6 templates/transparent export; optical flow, motion
blur, Bezier evaluation (MO0 §11); clock/iris/gradient wipes; per-effect
blending; the whole inherited key-editor bundle (drag/move, multi-key edit,
key copy/paste, on-canvas position path — all reassigned to MO6 per N6 R-D,
which lands before the production session); the `portrait_feed` profile (a
standalone DL1 slice per N6 R-E); Kani (all new maths is float pixel paths —
differential fixtures only, per MO0 §3).

## 1 Background and production context

MO0 §4 fixes the shape: tracks stay the document model, `CompositorLayer`
stays a per-frame artifact, `Normal` stays fixed-function, everything else
splits into a scene-linear sub-composite re-entering via `input_linear`,
each feature with a CPU twin. MO1 shipped the base: completed `transform`,
`Image` stills as freeze clips, both `enabled` flags, keep-outside
survival, single-key ops, `CopyClipAttributes`, `plan_motion`, i64 kernels.

Production order (Riel 2026-09-25) puts MO2 next: AW1 S1 → MO2 → AW2, CC8
S1 in parallel, CC8 S2/S3 after MO2's matching parts (N6 R-A). The one
hands-on session produces two videos — a 16:9 presenter-led AI explainer
(presenter, diagram cutaways, callouts, PiP, title cards) and a MockingBoard
Instagram piece (9:16 Reel + 4:5 feed cut) — after an agent dry run of both.
Fixtures mirror those jobs: PiP (`Normal`), `Screen` leaks, `Multiply`
callouts, a section look, solids under type, push/slide beats.

## 2 Data model

**R1 [core] (blend mode).** New `BlendMode` enum — `Normal` (default),
`Multiply`, `Screen`, `Overlay`, `Darken`, `Lighten`, `Add` — as a `Clip`
field (blending happens between layers, MO0 §4),
`#[serde(default, skip_serializing_if = Normal)]`. Old readers ignore it
(no `deny_unknown_fields`); operations take it typed, so no
`UnknownBlendMode` can exist post-parse. Honoured only for visual layers;
silently inert on audio-only ones (the kind-explicit alternative would
strand linked A/V batches). Pinned by serde-identity + inert-audio tests.

**R2 [core] (adjustment).** New unit variant `ClipContent::Adjustment`: no
asset (`asset` ignored like `Title`), `source_range` is a project-frame span,
no audio (`timeline_audio_segments` already skips non-`Media`), full
`effects` + `enabled` + `enabled_curve` + `transition_in`. The look is the
clip's own effect stack evaluated at clip-local frames.

**R3 [core/media] (solid).** New `ClipContent::Solid(SolidColor)` with
`SolidColor { r: u8, g: u8, b: u8 }` display-coded bytes. The fill is built
as a display `FrameTexture` and enters working space through the same
`WorkingFrame::from_display_frame` conversion titles use (N6 R-A) — never a
private sRGB inverse (B6). Span/audio/asset rules as R2. Opaque
(`alpha = 1`); the full-frame-quad path stretches it, so transform,
opacity, keys, and blend apply untouched. Pinned by the R-A seam test
(solid/title same-colour identity) plus coloured-solid and mid-grey pins.
(Hdr2020/context-change integration tests belong to CC8 S3, P6.)

**R4 [core] (transitions).** `Transition { name, duration }` is unchanged;
`TransitionShading` gains `Push { axis, sign }`, `Slide { axis, sign }`,
`Wipe { axis, sign }`, and `TRANSITION_DESCRIPTORS` grows 12 rows
(`push/slide/wipe × left/right/up/down`). Direction names the entry edge
(`push_left`: entering from the left, below-stack exits right). Validation
reuses `UnknownTransition` / `InvalidTransitionDuration`; the
descriptor-count test is extended, not forked.

**R5 [core] (span integrity).** The MO1 R9 span-or-media relaxation of
`require_media` extends to `Adjustment`/`Solid`: roll/slide move spans with
project-frame arithmetic; trim/split/move/ripple ride the span paths; speed
refused (`SpeedOnNonMediaClip`); `ReplaceClip`/`FitToFill` require `Media`;
relink vacuous (missing-asset arm); derived mappings skip both kinds (M23
precedent). Zero new `OpError` variants here.

**R6 [core] (survival).** New-kind effect/enable curves are keep-outside
video owners (MO1 R11–R13): trim shifts, split copies, `validate_ordered`
admits negatives, evaluation clamps. Pinned by trim-cycle + split tests.

**R7 [core] (format version).** One shared mechanism after AW1 S1, owned by
whichever of MO2 Part A / CC8 S2 lands first (expected MO2; N6 R-B):
`PROJECT_FORMAT_VERSION` means **maximum supported**; pure
`min_required_format_version` (in `kinewright-project`, beside the writer)
is used only for writing and returns 2 iff the document uses
`Adjustment`/`Solid` content, a non-`Normal` blend, or a transition name
outside the M20 three — else 1. **v2 is the union of MO2's and CC8's
disjuncts**: CC8 S2 adds its disjuncts to this same predicate (no rival
predicate, no second "format 2"). `can_overwrite_save` compares the
retained file version against max supported. Release rule: no release tag
between the first v2 landing and CC8 S2 (none has ever been cut); if one
is needed, the later incompatible expansion takes v3. Honest old-reader
behaviour: v1 readers fail unknown `ClipContent` variants at parse (before
any advisory handling); v1 files not using MO2 features write byte-identical
(pinned against pre-change bytes). Project saves, Save As and serialized
evaluation fixtures use the shared minimum-required predicate; after
successful saving, session version bookkeeping records the version actually
written. Recovery journals retain `writer_format_version =
PROJECT_FORMAT_VERSION`, independently of the journal format version,
because later appended operations may require features absent from the
initial snapshot; do not downgrade that header from the initial document's
predicate. Test an initially v1-compatible journal followed by a MO2
operation. Actual old-reader tests distinguish unknown-content parse
failure, unsupported-transition validation failure, and advisory opening
with overwrite refusal for otherwise parseable newer-format files;
universal open refusal is not required. Keep the union-v2/no-release-tag
rule unchanged.

**R8 [core] (operations).** Four new `Operation` variants, revision-gated,
one undo step each, as generated mutators (served quad untouched, §7):
`SetClipBlendMode { clip, blend_mode }`,
`AddAdjustmentClip { track, timeline_start, duration, effects }`,
`AddSolidClip { track, timeline_start, duration, color }`,
`SetSolidColor { clip, color }`. Validation matrix (S4): creation on an
audio track → new `AdjustmentOnAudioTrack` / `SolidOnAudioTrack` (the
`TitleOnAudioTrack` shape — `IncompatibleTrack` needs an asset id these
clips lack); audio setters on the new kinds → new `AdjustmentClipHasNoAudio`
/ `SolidClipHasNoAudio` in the three content-match arms beside
`TitleClipHasNoAudio`; `SetSolidColor` on a non-`Solid` →
`SolidColorOnNonSolidClip`; `chroma_key` added to an adjustment →
`EffectUnsupportedOnAdjustment`; colour-fade transitions onto an adjustment
→ `TransitionUnsupportedOnAdjustment` (table §5 R18). Creation ops map to
subject `Track` (the `AddTitle` precedent); the setters to `Clip`. New
`OpError`s join the compile-forced family table as `Placement` (track) or
`Malformed` (semantic); the investigator code allowlist stays at 54 — zero
new `IncidentCode`s.

Creation also validates existing track identity, non-negative start,
positive duration, checked end arithmetic, effects and overlap before
atomic commit; use the existing `MissingTrack`, `NegativeTimelinePosition`,
`InvalidSourceRange { start: 0, end: duration }`, `TimeOverflow` and overlap
errors where applicable. Adjustment support restrictions are document
invariants checked for initial effects, Add/InsertEffect,
CopyClipAttributes, transition setters, loading, replay and
candidate-document validation, including disabled unsupported effects. Core,
GUI and agent preserve the same typed refusal; render entry rejects invalid
documents defensively. Failed operations leave document, revision and undo
history unchanged.

## 3 Blend model (linear-light equations)

Inputs are scene-linear working values: `S` = the layer's graded src RGB,
`D` = the below-stack dst. `S`/`D` may exceed 1.0 (HDR highlights, `Add`)
or go negative (graded excursions). `αs` is the final source alpha produced
by the existing fragment chain: sampled alpha and folded opacity/title/
transition alpha, keying, the `fade_mix > 0` override to 1, then crop and
mask, followed by MO2 geometric coverage exactly once. Preserve this order.

**R9 [media] (equations).** `Normal`/`Multiply`/`Darken`/`Lighten`/`Add`
are algebraic (`S`, `S·D`, `min`, `max`, `S+D`); `Multiply`'s signed-domain
non-monotonicity (`(−1)·(−1) = 1`) is acknowledged and kept. `Screen` and
`Overlay` use the extended-domain form with `c(x) = clamp(x,0,1)`:
`B(S,D) = b(c(S),c(D)) + (S−c(S)) + (D−c(D))`, where `b` is the unit-domain
formula (`Screen`: `1−(1−S)(1−D)`; `Overlay`: `D≤0.5 ? 2SD :
1−2(1−S)(1−D)`, branched on `c(D)`). Excursions survive as additive
residuals, the working buffer is never clamped, and response stays
monotone above white. `Overlay`'s pivot is **half scene-linear reference
white**, not display mid-grey. Independent vectors (each pinned on both
lanes, §9): `Screen(0.5,0.5)=0.75`, `Screen(2,2)=3`, `Screen(3,3)=5`,
`Screen(2,0.5)=2`, `Screen(−1,0.5)=−0.5`, `Screen(−1,−1)=−2`,
`Screen(0.999,1.001)=1.001` (white-crossing continuity),
`Screen(0.18,4)=4`, `Overlay(0.75,0.4)=0.6`, `Overlay(0.75,0.5)=0.75`
(both branches agree), `Overlay(0.75,0.6)=0.8`, `Overlay(0.75,2)=2`,
`Overlay(2,0.4)=1.8`, `Overlay(0.75,−1)=−1`, `Overlay(0.18,4)=4`,
`Multiply(−1,−1)=1`, `Add(2,2)=4`, `Darken(2,−1)=−1`, `Lighten(2,−1)=2`.

**R9b [media] (alpha).** Verified against `Compositor::new`'s blend state
(`SrcAlpha`/`OneMinusSrcAlpha` colour, `One`/`OneMinusSrcAlpha` alpha) and
`fragment_main` (returns straight RGB + processed alpha): with a
transparent clear the target would accumulate premultiplied RGB. MO2
therefore declares the accumulator **opaque-over-black** (clear alpha 1,
as today) and special blends emit `(B(S,D), αs)` into the existing
fixed-function over — emitting the already-composited result would apply
alpha twice (B2). Composite: `out = αs·B + (1−αs)·D`, `out_a = 1`
(provably: `αs + 1·(1−αs)`). Vector: `Screen` S=0.75 D=0.5 αs=0.5 →
B=0.875, out=0.6875, alpha 1. Any future transparent accumulator needs an
explicit premultiplied contract and an alpha-aware equation — forbidden
without a new design. Pinned by an opaque-accumulator assert (every
accumulator readback alpha ≡ 1) plus the vector. Pin a partially
transparent colour-fade source with mask and non-Normal blending.

**R10 [media] (domain).** No clamp inside blend (the CC1
no-intermediate-clamp invariant survives MO2). MO2 refuses non-finite
working values and overflow at an f16 storage boundary; it never accepts
adapter saturation as success. Detect invalid values before storage and
retain a sticky per-layer failure indication until readback, including
failures subsequently covered by another layer. Return a typed `MediaError`
carrying offending clip and project frame through working, monitor,
delivery and proof paths; map it through the existing incident family
without adding an investigator code. Representable values use
round-to-nearest f16 storage. Pin `Add(40000,40000)` and
`Multiply(256,256)` at full alpha as overflow refusals, forced NaN/inf
refusals, and representable `Add(2,2)=4` preservation on every backend.

**R11 [media] (working-space agnostic).** The equations are per-channel and
the accumulator/branch code holds no primaries constants: blending happens
in whatever linear working buffer the compositor hands it. The twin takes
working buffers as given. CC8 S3 threads working reference and insertion
intent through generated-content conversion, applicable colour kernels,
render/readback context and cache keys; MO2 blend equations remain
unchanged. MO2 tests equation equality for identical supplied
working-linear buffers. It does not assert that identical display-coded
ramps produce identical working values under different insertion
intents/spaces. Solid/title equality compares matching opaque source
colours through the shared conversion. Executable Hdr2020/context-change
integration tests belong to CC8 S3. Add raw-working peak vectors
`Screen(P,P)=Overlay(P,P)=2P−1`, including `P=3.776475 → 6.552950` and
`P=46.4159 → 91.8318`, with actual storage rounding modelled separately.

## 4 Sub-composite pass

`composite()` today draws every layer bottom-to-top in one pass
(`LoadOp::Clear(BLACK)`, fixed-function over) and cannot rebind the output
as input — hence the split (MO0 §4).

**R12 [media] (branch rule).** The fast path runs iff every resolved layer
is `Normal`, no layer is `Adjustment` content, and no layer carries a `Push`
transition (`Slide`/`Wipe` are uniform-only — coverage + offset — and stay
in the pass). The fast path is today's draw sequence with neutral new
uniforms — no copy, no per-frame allocation beyond today's. The shared
layout adds descriptors but no additional bind-group-setting commands for
`Normal` draws. The branch decides per frame on resolved layers (disabled
clips already excluded), so one blend clip keeps the fast path on all
unaffected frames. Normal-only frames before, between and after special
intervals require zero accumulator copies; Normal draws inside a split
frame add zero copies.

**R13 [media] (slow path).** Let `D0` be the complete pre-special output.
Ordinary blend/adjustment layers snapshot it once. Push first snapshots
`D0`, then draws a full-raster opaque backdrop: for displacement `q`,
sample `D0(x−q)` when in bounds and **unshifted `D0(x)` otherwise**. This
defines what entering transparency reveals and restores the ordinary
below-stack at completion; never translate the backdrop quad or clamp edge
samples. A non-Normal entering layer then re-snapshots this shifted
backdrop and blends against it. An adjustment's source remains the
original `D0`, with its colour stack evaluated once; for non-Normal
adjustment Push, preserve `D0` in a second pooled texture before
overwriting the destination snapshot. Copy counts are ordinary special 1,
Normal Push 1, non-Normal Push 2, and non-Normal adjustment Push 3. Ledger
the extra preserved source. Pin transparent/masked/transformed endpoints,
OOB fallback, adjustment Push, and exact completion equality with ordinary
composition.

**R14 [media] (ABI).** `LayerParams`/WGSL grow exactly 4 words — 208 → 224
bytes asserted by an exact layout/size test: `blend_mode` selector (0 =
`Normal`, accumulator sample under guard) plus transition-coverage words
(edge, axis, on) shared by wipe/slide/push; `Push`/`Slide` offsets fold
host-side into the offset arms; output-space coverage uses the fragment's
pixel position (ME5). One pipeline, one bind-group
layout. Budget one additional writable storage binding for per-layer
validity flags: three sampled textures, two samplers, one uniform, two
storage buffers. Identity is recovered from the layer-to-clip mapping; the
four-word uniform extension remains 224 bytes. Assert the actual layout
and backend limits, and include validity resources in
allocation/performance accounting. The accumulator slot is always bound
via one cached pooled 1×1 dummy on the fast path. `is_pixel_exact_blit`
extends: coverage on or transition offset ≠ 0 forces the filtering
sampler. The dummy adds descriptors but proves nothing — byte identity is
proven by exact pre/post buffer comparison (R15), not by the binding
shape (S6).

**R15 [media] (Normal identity).** Same-backend pre/post `Normal` renders
are byte-identical — a *regression pin* (§13 pins), not a fail-today gate:
an ABI change does not make a correct identity test fail (B8).
Same-backend Normal identity compares complete pre/post working and
monitor buffers exactly under the same adapter, driver and inputs;
toleranced probes are supplementary and cross-backend comparisons remain
toleranced. No SHA-256 frame pins anywhere in MO2 (N4 G5). Every other
mode matches the twin within §9 tolerances.

**R16 [media] (CPU twin).** An explicit raster/sampling reference (B8): the
twin reuses `visual_layers_at`, decode and `params_for`. The CPU twin
reuses unquantized CPU colour kernels such as `apply_color_nodes_at`, not
the already-quantizing `cpu_reference_linear`/`cpu_reference_monitor`
wrappers as intermediate operators. It independently reproduces
rasterization, sampling, legacy processing, key/spill, fades, crop, mask,
coverage and blending; round only at actual source/target storage
boundaries, with texture copies preserving bits. Tolerances per domain
(§9): unit-domain absolute; over-range relative/ULP; monitor bytes
max/p99/mean — all MO2-specific per R27. Retain intentional formula,
pixel and geometry mutations that demonstrably fail. The twin also
re-gates the MO1 R26 goldens, discharging the MO1 §11 deferral.

## 5 Adjustment clips

**R17 [core] (scope).** An adjustment affects the composite of strictly
lower tracks over its own span (Premiere) — never same-track neighbours or
layers above. Stacks compose bottom-to-top, each seeing the adjustments
below it. Pinned by a two-adjustment order-swap test.

**R18 [core/media] (render).** An adjustment draws as
`result = blend_over(G(below), below, strength, adjustment.blend_mode)`:
sample the accumulator (`input_linear` re-entry), run the supported stack
`G`, composite with the adjustment's own blend mode at strength (B4 — the
blend field is honoured, not forced `Normal`). Adjustment strength is its
resulting alpha, including opacity, mask, crop, transition coverage and
transformed geometry; a disabled adjustment contributes no layer. Support
table (unsupported = typed refusal
at write time, identically in core, GUI, and agent — never silent):

| Feature on adjustment | Verdict |
|---|---|
| Colour nodes, primary/wheels/curves, LUT, `brightness`/`contrast`/`saturation` | Supported (the look) |
| `opacity` | Supported (= strength) |
| `transform`/`crop`/`reframe` | Supported (reframe the graded image) |
| `mask` | Supported (= strength mask) |
| `blend_mode` ≠ `Normal` | Supported (the outer `blend_over`) |
| `crossfade`/`push`/`slide`/`wipe` | Supported (alpha/geometry on the graded image) |
| `chroma_key` | Refused: `EffectUnsupportedOnAdjustment` (needs foreground semantics; MO5 revisits) |
| `fade_from_black`/`fade_from_white` | Refused: `TransitionUnsupportedOnAdjustment` (M20 opaque-fade semantics would occlude the below-stack the look needs) |

Pinned by: two stacked adjustments (order swap), zero/half/full opacity,
keyed enable, mask, transform, empty below-stack, upper-track invariance —
each GPU + twin.

**R19 [core/media] (enable + key home).** Clip `enabled`/`enabled_curve`
(MO1 R5) apply: a disabled adjustment is skipped in `visual_layers_at`,
byte-identical to removed; keyframed `enabled` cuts at the key's first
frame. Per-effect `enabled` reuses the existing skip arms. Key-home erratum
(S2): adjustment effect keys are **clip-local** (R6); MO0 §3's "adjustment
keys in project frames" is corrected to bus/master only.

## 6 Transitions

Transition-in-only is kept (M20): each transition covers the entering
clip's first `duration` frames over the static-or-pushed below-stack. No
outgoing-clip model, no centered transitions.

**R20 [media] (progress + endpoints).** For duration `d>1`,
`p=offset/(d−1)`. Geometric transitions are active only before
`offset=d−1`; at that frame and thereafter use the ordinary authored layer
contribution. Duration 1 is an identity. Endpoints are NOT uniform (S1):
`crossfade` and the 12 geometric transitions start with the entering clip
invisible. M20 colour-fade endpoint statements describe source
colour/alpha before crop, mask and layer blending. Pinned per descriptor,
incl. transformed and translucent titles.

**R21 [media] (coverage + midpoint).** Coverage is output-space and
independent of authored transform (S1): the entering layer is clipped to
the revealed region (alpha-multiplied coverage from the R14 words, never
`discard`), so a scaled-up or displaced clip is still invisible at `p=0`
and exactly revealed-region-shaped throughout. In screen fractions,
positive right/down, entering/backdrop offsets are: left `(-(1−p),0)/
(p,0)`; right `((1−p),0)/(-p,0)`; up `(0,-(1−p))/(0,p)`; down
`(0,(1−p))/(0,-p)`. Slide uses only the entering offset; Wipe uses neither
offset. Coverage is left `x<p`, right `x≥1−p`, up `y<p`, down `y≥1−p`,
using output pixel centres. Coverage clips authored geometry; it does not
fill transparent or uncovered source pixels. Exact half-frame image
assertions use opaque, untransformed full-frame fixtures and even
dimensions on the transition axis. Midpoint fixtures use **odd durations**
(exact `p = 0.5` at the middle offset). Test transformed/translucent
sources separately, including the last transition frame and following
frame.

**R22 [media] (audio).** The M20 linear gain ramp applies verbatim to all
12 new transitions, type-independent. Pinned by one ramp-identity test
across all 15 descriptors, plus linked-audio and disabled-clip cases.

**R23 [media] (composition).** Geometry → coverage → grade stack →
blend-over, in that order: a blend-mode clip under `Push`/`Slide`/`Wipe`
blends at its drawn pixels against the (possibly shifted) below-stack;
push-blend takes the R13 re-snapshot, and non-Normal adjustment Push
preserves `D0` in a second pooled texture per R13. `Push` always splits
the pass (R12);
`Slide`/`Wipe` on a `Normal` clip stay fixed-function. Opacity/keys/
`enabled` compose through the existing `params_for` arms. Pinned by a
`Screen`-under-`push_left` midpoint golden on both lanes.

## 7 `preview_solo`

**R24 [agent] (capability).** `preview_solo` is a registry capability
reached through `invoke_capability` — never a served tool (MO0 §7).
Args: `clip`, `expected_revision`, `samples` (2..16, default 8),
`context: isolated | below` (default by blend: `Normal`→`isolated`,
blend→`below`; `Adjustment` is always `below`, no override — an
isolated adjustment is meaningless), `full_res: bool` (default false:
the MO0 §7 small-text fallback, N6 R-F). Full-resolution mode instead
emits one midpoint `floor((L−1)/2)`, paired for adjustments. It renders
against a revision-checked immutable snapshot via the proof path
(`monitor_proof_for_document` family) over the clip's span:
`isolated` = the layer alone over black; `below` = the layer over its
true below-stack with above-layers hidden. For span length `L>0`, emit
`k=min(requested,L,cell_limit)` sample times, where `cell_limit=16`
normally and 8 for adjustment pairs. For `k>1`, local offsets are
`floor(i×(L−1)/(k−1))`; for `k=1`, use 0. Report requested/emitted counts
and inactive cells. Sampled frames where the clip is disabled yield
inactive cells with reasons, never silent gaps. Failures use a dedicated
`SoloError` enum
(`SoloClipNotVisible`, `SoloWindowEmpty`, `SoloOverBudget` — S4:
`MatteProofError::ClipNotVisible` is matte-specific), preserved typed
through GUI/headless seams. Response = strip PNG (`ContentBlock::image`)
+ sampling-report JSON (frames, resolution, **actual backend/provenance**,
context, per-sample hashes). Target is a clip id, so MO3 compounds ride
free. Pinned by real-endpoint tests incl. pairs, duplicates, inactive
cells, and the fallback.

**R25 [agent] (budgets + registry).** Report JSON ≤ 4 KiB; strip PNG ≤
768 KiB raw at 320 px thumbs (≤16 cells; pairs count double); **total
serialized response ≤ 1,056 KiB** measured on the wire (768 KiB × 4/3
base64 + JSON + envelope headroom — S3). Thumbnail cells preserve aspect
within 320×640 pixels; at most 16 cells and 3,276,800 decoded pixels are
admitted. Degrade sample count to `min(2,L)`, then halve thumbnail bounds
to 160×320, keeping pairs together. Full-resolution cells retain the
working raster, with each dimension ≤8192 and total decoded pixels
≤16,777,216; never silently downsample this mode. All modes obey the raw
PNG, report and complete serialized-response ceilings; an unsatisfied
limit returns JSON-only `SoloOverBudget`. Full-resolution mode does not
inherit the two-sample minimum. Description ≤ 1,024 B, load-bearing
clause first. The served quad stays byte-identical (7 / 5,660 B / 3,510 B
/ 998 B). AW1 S1 supplies shared IO; its five lifecycle inspectors arrive
in S3. Measure the merged registry before each publishing commit.
Relative to that surface, Part A adds `(4,4,0)` and Part B adds `(1,0,1)`
in total/generated/inspector order. Without AW1 S3 or CC8 deltas, counts
are `148/60/88 → 152/64/88 → 153/64/89`; after AW1 S3 they are
`153/60/93 → 157/64/93 → 158/64/94`. Compose actual intervening changes
and advance the measurement ordinal from the merged ledger. Update
`schema.rs`'s operation-name test and `INSPECTOR_TOOL_NAMES`; re-pin all
six metrics in
`served_surface_is_small_and_keeps_the_internal_registry_discoverable`,
both endpoint tests `cc7_the_agent_surface_is_unchanged_by_this_slice`
and the current
`mo1_the_served_quad_does_not_move_for_the_twenty_first_measurement`,
plus M36. Assert the unchanged served quad. R25 registry duties belong to
both parts; solo transport/budget duties belong to Part B. Reuse AW1's
shared serializer and regression tests.

## 8 Person GUI with agent parity

**R26 [app] (surface).** Every §2/§7 capability has a GUI sender (MO0 §7),
pinned by an op↔GUI checklist test in the N5 L4 style (senders exercised
through the real widget/menu seam): clip-header blend dropdown (7 modes);
"New adjustment clip" / "New solid" menus; solid colour editor; 15-name
transition menus; timeline wedges with direction glyphs; solo button with
strip dialog (pairs for adjustments, full-res on request); overlay drag
for `transform` position/scale (CC5 precedent), one undo step via the MO1
R22 gesture pattern. Adjustment span is
the selected clips' enclosing timeline range when selection exists;
otherwise both generated kinds default to `max(1, round(2×project_fps))`
frames from the playhead. An adjustment's destination must be above every
intended affected track and free across its span. Honour an explicit
eligible target track; otherwise choose the lowest-index eligible free
video track. A solid requires only a free video track. With no eligible
track, return a typed placement refusal describing the span and required
position, without inventing a track ID or creating a track. The whole
key-editor bundle moves to MO6 per N6 R-D (§14).

## 9 Verification: lanes, tolerances, perf

**R27 [media] (lanes + tolerances).** Every render gate (§13) names its
lane: lavapipe GPU, CPU twin, or both (MO0 §8). Weight sits on GPU≡twin
differentials and contract tests; no SHA-256 frame pins (N4 G5). Pinned
tolerances per domain (B8): unit-domain working-linear max abs ≤ 1e-3/
channel; over-range relative ≤ 2^-10 and ≤ 4 f16 ULP; monitor bytes max ≤
2 codes, p99 ≤ 1, mean ≤ 0.25 (the CC1 `abs_code_diff_rgb` method).
R27's tolerances are MO2-specific; the future CC8 column additionally
satisfies PB1–PB4. Fixtures are production-flavoured:
PiP-over-presenter (`Normal`), `Screen` light leak, `Multiply` callout,
adjustment look, solid title card, push/slide/wipe midpoints, and the §3
extended-domain vectors — each GPU + twin except where §13 says
otherwise. The MO1 R26 goldens are re-gated against the twin here, with
old reference results preserved where they exist.

**R28 [media] (perf).** `TexturePool.bytes` counts idle textures only and
`performance_workloads` measures seek latency/RSS — neither bounds MO2's
promises (B7). Absolute floors: lavapipe ≥ 8 fps, Windows/WARP ≥ 20 fps,
RTX 3090 ≥ 60 fps, p95 ≤ 3× mean. The `blend_heavy` workload is 4-track
1080p: PiP + `Screen` overlay + adjustment + `push`. Measure sequential
completed preview
frames, including decode/render/readback, with one frame in flight: 30
warmup frames followed by 300 measured frames, three runs per
workload/backend. Record actual output dimensions, adapter/driver, mean
fps, mean frame time and p95 frame time; p95 must be ≤3× mean frame time.
Apply the stated absolute floors to 1920×1080 typical and blend-heavy
workloads. Capture pre-MO2 Normal baselines with this same protocol;
retain the 5% regression limit and also exercise the existing 4K heavy
workload. Ledger textures, buffers, staging/readback, validity flags and
retained in-flight resources without double counting; ceilings are 384
MiB at 1080p and 1,536 MiB at 4K. RSS is corroborating process-memory
evidence, not a GPU-allocation measurement. The slowdown control injects
per-frame delay greater than twice the backend's frame-time floor and must
fail throughput. The ledger control submits four 4096×4096 RGBA16F
reservations, totalling 512 MiB, and must reject the 1080p budget. Extra
copies and an unspecified 8K texture alone are not guaranteed failing
controls. Report solo peak resources and elapsed time. Part B measures on
all three backends and pins; a miss optimises copies, never tolerances.

## 10 Incidents

**R29 [core] (mapping).** Zero new `IncidentCode`s; the investigator code
allowlist stays at 54. The seven R8 variants join the compile-forced
(variant → `IncidentFamily`) table as `Placement` (two on-audio-track) or
`Malformed` (five semantic), with policy/recovery rows where required.
Reused: `UnknownTransition` / `InvalidTransitionDuration`,
`SpeedOnNonMediaClip`, `EditorialRequiresMedia`, keep-outside/enable
machinery, v1/v2 refusal. Solo failures are `SoloError` (R24), not
incidents. Pinned by table tests + one render test per reused arm.

## 11 Concurrency (AW1 / CC8 / AW2)

**R30 [core/agent] (AW1).** AW1 is in review; MO2 merges after AW1 S1. The
R7 writer change lands in the AW1-moved `kinewright-project` location (not
a resurrected app copy), owned by the first lander per N6 R-B, and the R25
sextuple re-pins serialise after AW1's lifecycle capabilities at the same
pin sites (AW1 S1 supplies shared IO; its five lifecycle inspectors arrive
in S3. Measure the merged registry before each publishing commit). No
logic overlap beyond the pins and the writer — no shared code, no joint
tests.

**R31 [media] (CC8).** Order per N6 R-A: AW1 S1 → MO2 → AW2; CC8 S1 in
parallel; CC8 S2/S3 after MO2's matching parts, rebasing onto them. MO2
stays corner-free: agnostic equations (R9/R11), generated-content solids
(R3); CC8 S3 makes that one path HDR-aware, adds the Hdr2020 column,
touches neither equation nor branch. MO2's SDR gates become CC8's
regression pins; v2 is the R7 union (N6 R-B no-tag rule). Combined gate:
CC8 S3 re-runs the MO2 suite as pins plus its Hdr2020 column first.

**R32 [media] (AW2 seam).** AW2 code clips rasterise CPU-side and alpha-over
like titles, so they need MO2's composite *order*, not its pixels. The
seam is a content-independent resolved-layer contract. Preserve clip
identity, project frame, layer role, blend, effects, transition and pixel
domain through resolution, decoding and compositor assembly. Adding
`blend` alone is insufficient: `CompositorLayer` currently lacks identity.
Supply an explicit identity mapping to compositor results and validity
flags so a render refusal identifies the offending clip/frame without
guessing from the final image. Distinguish adjustment instructions from
ordinary pixel sources while keeping generated-content handling
independent of future `Code` naming. AW2 adds one `TimelineVisualLayer`
kind flowing through the unchanged branch; `Clip.blend_mode` honours it
free (R1). MO2 pins the seam by driving transparent generated pixels
(varying alpha, non-`Normal` blend) through the real `compositor_layers` +
branch + twin assembly — a stub enum kind alone proves nothing (S7). No
MO2 code names `Code`.

## 12 Staging, line budget, per-commit gates

Two parts, two commits (MO0 §8: large), **landed together** (the N3
precedent — Part A alone would expose renderable content without
rendering, so it never sits alone on main; its `visual_layers_at` arms
fail closed typed until Part B lands). Production budgets count
new-or-changed production lines (AW1/CC8 precedent); tests are required
per rule and counted separately.

- **Part A — core model + registry** (core, agent schema, project writer):
  R1–R8, R17/R19/R29 model halves, R25 registry half, R30 writer + pins.
  New `ClipContent`/`TransitionShading` variants force every match arm —
  the compiler lists the edit sites. Gate: contract + survival + serde +
  table tests, sextuple re-pinned (Part A adds `(4,4,0)` in
  total/generated/inspector order over the measured merged surface),
  served quad green.
- **Part B — render + person + solo** (`kinewright-media`,
  `kinewright-app`, `kinewright-agent` capability): R9–R16, R18,
  R20–R28, M36 ledger. Decomposed (S7): raster/sampling twin
  ≤ 700, pass scheduling + WGSL ≤ 500, GUI gestures + menus ≤ 400,
  solo transport ≤ 250, benchmarks + ledger ≤ 150. Gate: all §13 gates
  green on named lanes, floors pinned on all three backends, sextuple
  re-pinned (Part B adds `(1,0,1)` in total/generated/inspector order),
  parity checklist green.

Line budget: ≤ 3,200 production lines total (A ≤ 1,200, B ≤ 2,000); a
part overrunning by > 20% splits instead of thinning. Per-commit gate:
the green workspace gate per AGENTS.md (`build`/`test --workspace`,
`fmt -- --check`, `clippy --workspace --all-targets -- -D warnings`)
plus two Opus reviews per part (MO0 §0). This doc holds ≤ ~720 lines.

## 13 Exit gates (named behaviours that fail today)

Each names its lane; "no regressions" alone gates nothing (MO0 §0).

1. `screen_light_leak_matches_twin` — `Screen` overlay matches the twin
   within R27 (fails: no blend). GPU + twin.
2. `multiply_callout_matches_twin` — `Multiply` callout matches within
   R27 (fails: no blend). GPU + twin.
3. `remaining_modes_match_twin` — `Overlay`/`Darken`/`Lighten`/`Add`
   table-driven against the twin, incl. every §3 extended-domain vector
   (negatives, white crossings, CC8 peaks) and an over-1.0 `Add`
   surviving to `render_working` (fails: no blend). GPU + twin.
4. `adjustment_look_over_section` — adjustment over two tracks equals
   grade→blend_over at zero/half/full strength; order-swap exact, upper
   tracks invariant (fails: no `Adjustment`). GPU + twin.
5. `push_midpoint_splits_frame` — Gate 5 tests all four Push directions
   with shifted probes (odd durations, exact `p = 0.5`): entering covers
   its half, below-stack probes shift per R21; OOB falls back to unshifted
   `D0(x)` through a translucent title; completion equals ordinary
   composition (fails: no push). GPU + twin.
6. `slide_and_wipe_midpoints` — Gate 6 tests the eight Slide/Wipe
   directions with stationary backdrops; scaled-up entering still
   invisible at `p=0` (fails: no slide/wipe). GPU + twin; all 15
   endpoints pinned alongside. Transformed/translucent sources tested
   separately, including the last transition frame and following frame.
7. `solid_title_card_renders` — solid + opacity + transform renders the
   pinned card; solid/title same-colour identity holds (fails: no
   `Solid`). GPU + twin.
8. `adjustment_solo_returns_pair` — adjustment solo returns same-sample
   before/after pairs + provenance report (fails: no solo). Real-endpoint.
9. `solo_strip_inside_byte_budget` — 16-sample solo fits 768 KiB raw /
   1,056 KiB serialized + 4 KiB JSON; over-budget degrades pairs-together
   samples-then-width, noted in the report (fails: no solo). Real-endpoint.
10. `blend_heavy_holds_floors` — the R28 workload holds the absolute fps
    floors + 5% baseline rule with live+idle bytes inside ceiling (384
    MiB at 1080p, 1,536 MiB at 4K) on all three backends; the slowdown
    control (per-frame delay greater than twice the backend's frame-time
    floor) fails throughput and the ledger control (four 4096×4096
    RGBA16F reservations, 512 MiB) rejects the 1080p budget (fails: no
    floors). Backend + ledger tests.
11. `v2_stamps_only_when_used` — MO2-feature files stamp 2 and reopen;
    the v1 corpus writes byte-identical and stays 1. Actual old-reader
    tests distinguish unknown-content parse failure,
    unsupported-transition validation failure, and advisory opening with
    overwrite refusal for otherwise parseable newer-format files;
    universal open refusal is not required (fails: no predicate).
    Contract test.

Pins (pass today, must not regress): `served_quad_unchanged`
(7 / 5,660 B / 3,510 B / 998 B, ordinal advanced from the merged ledger);
`normal_pre_post_identity` — same-backend pre/post `Normal` renders
byte-identical by exact complete working and monitor buffer comparison
under the same adapter, driver and inputs, with a fast-path probe
asserting zero accumulator copies (B8); `mo1_transform_r26` re-gated
against the twin (MO1 §11 discharged); `cc8_g2_sdr_identity` through the
MO2 window.

## 14 Deferrals with owners

- Clock/iris/gradient wipes; wipe feather → future transition-depth work.
- `chroma_key` on adjustments; mask windows → MO5 (single-shape strength
  mask here, R18).
- Per-effect blending; track-level effect stacks → rejected (MO0 §4),
  not deferred.
- The whole key-editor bundle (drag/move, multi-key edit, key copy/paste,
  on-canvas position path) → MO6, explicit reassignment per N6 R-D (MO6
  lands before the production session); recorded in the MO6 row later.
- `portrait_feed` delivery profile → standalone DL1 slice per N6 R-E.
- Hdr2020 blend/adjust goldens → CC8 S3 (equations unchanged, R11/R31).
- `Code`-layer blend interplay → AW2 (rides the R32 seam).
- Bezier evaluation/handles, transparent export, presets → MO6.
- Ping-pong accumulator, below-stack motion blur → future perf work (the
  R13 copy counts — ordinary special 1, Normal Push 1, non-Normal Push 2,
  non-Normal adjustment Push 3 — stand until a floor says otherwise).
- Multi-turn rotation, off-layer pivots, >8192 px stills, image
  sequences → unchanged from MO1 §11.

## 15 Production-session checklist material

No per-slice hands-on: this section is material for the one production
session (and its agent dry run), not a slice gate.

Explainer (16:9): PiP over diagram cutaways (identity pin); `Screen` leak
(gate 1); `Multiply` callouts (gate 2); one section look, soloed
before/after (gates 4, 8); solid title cards (gate 7); push/slide beats
(gates 5–6). Reel (9:16 + 4:5): same stack recomposed — animated type on
solids, push/slide beats, wipes at breaks, both portrait ratios. Dry run:
agent builds both cuts via `plan_motion` + MO2 mutators + `preview_solo`,
inside budgets; Riel answers creative questions only (MO0 §9): believable
PiP, look holding across cuts, pushes landing on beats.

## R Rule→gate trace

- [core] R1–R8 (model/ops/format) → gates 1–7, 11 + Part-A contract tests.
- [media] R9–R11 (equations/domain/space) → gates 1–3, 7.
- [media] R12–R15 (pass/identity) → pin `normal_pre_post_identity`, gates 5–6, 10.
- [media] R16/R27 (twin/lanes) → every GPU+twin gate + the MO1-R26 re-gate pin.
- [core/media] R17–R19 (adjustment) → gates 4, 8.
- [media] R20–R23 (transitions) → gates 5–6.
- [agent] R24–R25 (solo/budgets/registry) → gates 8–9 + served-quad pin (R25 registry both parts, transport Part B).
- [app] R26 (GUI) → parity checklist + dry run.
- [media] R28 (perf) → gate 10.
- [core] R29 (incidents) → table tests.
- [core/agent/media] R30–R32 → pins + seam test.
