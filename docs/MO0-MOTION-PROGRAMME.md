# MO0 — Motion, compositing and retiming programme design

Status: **promoted 2026-09-23** (revision 2) — incorporates the Opus critic verdict
(`revise`), the lead's N1 rulings, and Riel's answers (all binding).
Decides the boundary, animation/compositing/retiming models,
titles/templates scope, agent surface, and slices MO1–MO7. Idea-level
inspiration only is taken from an agent-first motion suite (Tesseract):
no copied material; every semantic is grounded in Premiere/After Effects
conventions and our own design.

Conventions: prose cites symbols (`Type`/`fn`/`const`/module names), never
`file:line`. Exhaustive inventories (descriptor tables, error variants, registry
counts) belong in compile-forced tests, not here. Numbers below are measured on
main unless marked as estimates.

## 0. How this programme runs

Recipe v2 mechanics, fixed for MO1–MO6:

- One design doc per slice (≤ ~600 lines), citing symbols not `file:line`;
  an Opus critic reviews each design; probes only for questions reading
  cannot settle.
- Stages ship with two Opus reviews each and a green workspace gate per
  commit; a hands-on session with Riel ends each slice, and the slice is
  not complete until its findings are discharged or recorded as deferrals.
- Exit gates are named behaviours that fail today, never "no regressions"
  alone.

## Changes in revision 2

- B1 → §4: sub-composite design, scene-linear blend, CPU-reference twin
  per feature, fps + VRAM gate. B2 → §4 + §8: compound addressing; MO3
  behind a call-site probe. B3 → §3: keys kept outside in/out (Riel
  ruled); audio keeps AU4 rules. S1 → §3: pure kernels, no-float rule,
  stage-0 Kani probe (the "non-saturating subtraction" side-claim was
  wrong — lead verified `saturating_sub` — and is not adopted).
- S2 → §5: `ClipTimeMap`. S3 → §11 + §4/§8: expressions to AE; enable
  flags in MO1. S4 → §8: all day-one jobs placed. S5 → §8 + §6:
  renumbered MO1–MO7; templates reuse `Sequence`. S6 → §8/§9:
  named-behaviour gates with lanes. S7 → §7: byte budgets, decimation,
  image-returning `preview_solo`, adjustment before/after pair.
- S8 → §1/§10/§11: honest gap labels, reserved tangents, decided
  questions recorded. Nits → §3/§4/§6 + roadmap draft. Tesseract scrub →
  whole doc (idea-level inspiration only, own words, Premiere/AE
  grounding).

## 1. Product boundary

**Recommendation.** "Premiere-class editorial compositing" is the motion a
picture editor needs inside the NLE, and nothing that belongs to a shot-based
compositing application. In scope for this programme:

- Keyframed motion on every clip: position, scale, rotation, anchor,
  opacity, crop — with easing and hold.
- Track-based compositing: ordered tracks, per-clip blend modes,
  adjustment layers, nested compound clips.
- Retiming: speed ramps and time remap on the same curve model, freeze
  holds, reverse, frame blending.
- Editorial mattes: keyframed shape masks, track mattes, chroma key
  polish, tracker-driven animation on `track_region`.
- Vector-only titles and graphics templates with editable fields.
- Stills as first-class timeline media (the Ken Burns job).

Deferred to the later After Effects programme (per "The outcome", not a
slice here): 3D layers and cameras, particles and simulation, mesh warps,
mesh/point tracking, paint and rotoscoping tools, node-graph compositing,
custom shader authoring (WGSL or otherwise), 3D mesh import and rigging,
and typed parameter expressions. Honestly labelled as deferred Premiere
gaps — wanted, intended, but out of this programme (see §11 for owners):
optical-flow retiming, Bezier keyframe handles (serde-compatible tangent
fields are reserved on `Keyframe` now so the later addition migrates
cleanly), motion blur, and person/segmentation mattes. No fixed-shutter
approximation ships here; a half motion-blur would be worse than none.

Generative assets: raster image and video models are deferred — this
programme generates no pixels from a model. What ships is vector and
code-generated only: titles, shapes, layout templates, and procedural
patterns (gradient ramps, vignettes, grain) whose parameters are typed,
inspectable, and deterministic. A template is a document fragment with
named editable fields, not a PNG.

**Rejected alternative.** Matching a full motion-graphics suite's surface
(3D transforms, procedural animation, custom shaders) in one programme.
Rejected because it collapses the Premiere/AE boundary the roadmap
deliberately keeps: Kinewright's gap is a picture editor who cannot yet
animate a push-in, and depth per slice is the strategy that closes it.

**Cost.** No new crates, no new model dependencies. GPU work stays inside
`Compositor`/`compositor.wgsl` plus the CC3 grade-buffer pattern. The main
cost is core-model breadth and three GUI editors (keyframes, rubber-band
retime, nest step in/out) — see §8 sizes.

## 2. Current foundation, measured

What exists on main (read, not assumed):

- **One curve type already.** `AutomationCurve` (a `Vec<Keyframe>` with
  `at: TimeCode`, `value: i64`, `interpolation`) evaluates in pure integer
  fixed point (`value_at`, `CURVE_SCALE = 1_000_000`) with five
  `KeyframeInterpolation` kinds (`Hold`, `Linear`, `EaseIn`, `EaseOut`,
  `EaseInOut`). `rebase_clip_curve` and `clamp_project_curve` implement the
  AU4 survival policy across trim/split/slip/roll/speed/ripple/relink.
- **Effect keyframes.** `Effect.keyframes: BTreeMap<String, AutomationCurve>`
  makes every registered parameter automatable; `SetEffectKeyframes`
  replaces one parameter's whole curve, `ClearEffectKeyframes` removes it;
  `Effect::evaluated_at` resolves a static effect per rendered frame. CC3
  adds the keyframing policy: scalars freely, curves under whole-curve
  `Hold` steps or point-wise interpolation at constant point count
  (`NonHoldKeyframeParameter`, `CurvePointCountAnimatedWithPoints`).
- **Audio automation (AU4).** Five curve owners: `Clip.audio_gain_curve`
  (clip-local), track gain/pan (`TrackMix`), bus and master gain. Shared
  per-sample evaluation in `AudioMixProcessor`, live retarget without
  re-cue, rubber-band clip editor, mixer `AUTOMATION` section, inspector
  keyframe list, `plan_audio_ducking` / `plan_clip_fades`.
- **Motion effects (6 descriptors).** `opacity` (1 param);
  `transform` (uniform `scale_percent` 1..400, `x_percent`/`y_percent`
  ±100); `crop` (4 insets 0..45, additive fold, clamped); `reframe`;
  `mask` (one shape token rect/ellipse, centre, width/height, feather,
  invert); `chroma_key` (6 params). No rotation, anchor point,
  non-uniform scale, blend mode, or bypass on any video effect.
- **Compositor.** `Compositor`/`CompositorLayer`/`params_for` fold
  multi-effect stacks into one `LayerParams` block; tracks composite in
  document order, last on top, under a single alpha-over `BlendState`.
  `FrameRenderer::render` is the one preview/export path, with
  `render_monitor` / `render_delivery` / `render_working` /
  `render_matte` stage variants and a ≤16-node grade storage buffer.
- **Speed.** `Clip.speed_percent` (10..1000, default 100) via the
  effective-fps principle: every mapping goes through `clip_effective_fps`
  / `speed_scaled_fps`. Audio muted at ≠100; no ramping (explicitly
  deferred in M23); derived mappings (transcript, silence, scenes) skip
  speeded clips.
- **Titles.** `Title` (text, font/colour tokens, 3 positions, scrim,
  fades, 3 `CaptionPreset`s) via `SetTitleParam`; `title_layout` is
  shared by preview/export/QA. Titles carry effects but no keyframe
  surface of their own and no animation.
- **Transitions.** 3 registered (`crossfade`, `fade_from_black`,
  `fade_from_white`), transition-in only, via `TRANSITION_DESCRIPTORS`.
  No wipes, slides, or motion transitions.
- **Tracker.** Agent `track_region` (normalized-SAD template match) plus
  `track_matte_window` (CC5, smoothed), `track_mask_region`,
  `track_reframe_subject`: all prepare keyframe operations and commit
  nothing. CC7 pins a tracked secondary with a one-sample occlusion.
- **Clip kinds.** `ClipContent` has 3 variants (`Media`, `Title`,
  `Freeze`); `MediaKind` has 3 (`Video`, `Audio`, `AudioVideo`). No
  compound/nested clip, no adjustment layer, no still-image asset.

Gaps this programme must close: no keyframe editor UI for video params;
no rotation/anchor/per-axis scale; no blend modes; no adjustment or
compound clips; no time remap; no track mattes; no stills; titles not
animatable; no per-effect or per-clip enable; no effect presets or
copy/paste attributes; no nest/unnest; no push/slide/wipe transitions.
The audit's "Keyframes / motion —" row is now half-stale (the model
exists; the motion surface does not).

## 3. The animation model

**Recommendation: one curve type, already written.** `AutomationCurve` /
`Keyframe` / `KeyframeInterpolation` is the programme's animation
representation for every animatable parameter — motion, colour, audio,
retime. CC3 and AU4 do not converge by rewrite; they already coexist as
instances of the same type (`Effect.keyframes` values, `Clip.audio_gain_curve`,
`TrackMix`/bus/master curves). MO1 extends the owners and the editors, not
the representation. New motion parameters (rotation, anchor, per-axis
scale, blend factor, mask geometry, time remap) are further `AutomationCurve`
owners under the existing validation (`Empty`, `NegativePosition`,
`Unordered`), survival (`rebase_clip_curve`, `clamp_project_curve`), and
per-frame resolution (`Effect::evaluated_at`) machinery.

**Keys survive outside in/out (Riel ruled, Premiere behaviour).** AU4
survival verbatim would destroy video keys: a head trim would reshape an
eased push-in and trim-in-then-out would lose keys. Instead, video
`Effect.keyframes` and `time_remap` curves keep out-of-range keys: trim
shifts only, split copies the curve to both halves (each shifted to its
local origin), evaluation clamps. Audio curves keep the AU4 rules; the
policy is per-owner, and `EffectKeyframeOutsideClip` is relaxed for video
owners only. Cost: a policy flag through the survival call sites plus
relaxation tests (the MO1 brief pins the edge cases).

**Interpolation kinds: keep the five, reserve tangent fields.** `Hold`,
`Linear`, `EaseIn`, `EaseOut`, `EaseInOut` stay the complete evaluated
set; finer shaping is expressed with intermediate keys. `Keyframe` gains
reserved serde-compatible tangent fields now (ignored by evaluation) so
Bezier handles — a deferred Premiere gap (§11), not AE scope — migrate
cleanly later. Rejected: evaluating handles now (AU4 §1.3 already
deferred per-key tangents; handles would double the keyframe-editor UI
cost). Cost: two ignored fields plus a reservation test; the enum gains
no variants, so every existing match stays exhaustive.

**Upsert by frame, not by stable ID.** Within a curve, `at` is already a
unique key by validation (`Unordered` rejects duplicates), so MO1 adds two
single-key operations — insert-or-replace at a frame and remove at a frame,
both revision-gated like every mutation — and keeps whole-curve
`SetEffectKeyframes` for planner-authored curves. Moving a key in time is
remove+insert in one atomic batch. Rejected: stable key IDs — they would
migrate every stored curve to buy nothing atomic batches do not already
provide. Cost: two small operations plus agent schema rows; no migration.

**Time base: integer frames, owner-local.** Effect and clip keys stay in
clip-local frames, bus/master/adjustment keys in project frames, exactly
as today. No sub-frame keys: sub-frame addressing would break the
integer-frame timing invariant the whole thesis rests on. Retiming keys
map integer project frames to rational source positions (§5), with the
rational confined to the mapping function. Cost: none — the invariant is
preserved, not extended.

**No expressions in this programme.** Follow-links and LFOs are deferred
to the AE programme (§11): cross-clip references dangle and cycle,
`Effect::evaluated_at` cannot see them, and they have no GUI path. What
MO1 ships instead is a per-effect `enabled` flag plus a clip enable
toggle: a disabled effect is skipped in `evaluated_at`/`params_for`, a
disabled clip is skipped in layer resolution, and conditional visibility
is expressed with `Hold` keys on the keyframe-able `enabled` flag.
Rejected: string expressions (untyped, uninspectable, hostile to
validated-`Operation` deterministic replay). Cost: two flags, skip arms,
and toggle UI — far less than an expression evaluator.

**Kani: pure kernels first, probe before gates.** The admission rule
(`#[cfg(kani)]` module doc in `incident.rs`) admits only proofs under
5 min / 6 GB, with symbolic strings and `BTreeMap` out of scope — so the
`AutomationCurve` methods fail admission as written (`&self` over `Vec`,
map-building survival, private `ease`, nonlinear `i128` mul/div). So the
programme factors slice-level pure kernels — `fn(&[Keyframe], i64)`
evaluation, sorted-merge survival, bounded ranges fitting `i64` — with
methods as thin wrappers, and bans f32/f64 from core motion maths
(trigonometry lives only on the GPU and CPU-reference lanes). MO1 stage 0
is a Kani probe verifying one kernel harness end to end before any slice
pins a harness count. One harness, one assert each; float pixel paths
stay with differential fixtures, never Kani.

## 4. The compositing model

**Tracks, not layers.** Kinewright keeps tracks as the document model;
compositor layers stay per-frame resolved artifacts (`CompositorLayer`),
never stored. Nesting and adjustment are clip kinds, not a parallel layer
tree. Rejected: a single-composition layer document — tracks plus story
graph are the NLE identity, and a second hierarchy would strand every
editorial operation. Cost: none; the track order (last on top) stays the
composite order.

**Sub-composite design (B1).** The compositor draws all layers in one
render pass (`LoadOp::Clear`) under fixed-function alpha-over, and the
output texture cannot be rebound as input — so a shader branch alone
cannot do blend modes, adjustment-below, or mattes. The design: `Normal`
layers stay in the fixed-function pass (keeping "Normal bit-identical"
honest); any other layer splits the pass, copying the below-stack into an
`Rgba16Float` accumulator that re-enters via `input_linear`, so colour
nodes apply unchanged. Blending is scene-linear (ruled). Each new feature
ships a CPU-reference twin (the CC3 lane pattern); MO2 pins a preview fps
floor plus a VRAM ceiling covering the accumulator.
Rejected: reading the destination texture (impossible on this backend)
and doing blends in display-referred space (breaks the managed pipeline).
Cost: the pass-splitting renderer work in MO2, one twin per feature, and
the fps/VRAM measurement harness.

**Adjustment layers.** New `ClipContent::Adjustment`: a clip with effects
and a timeline range but no asset, applying its evaluated effects to the
composite of everything below it on lower tracks (the Premiere adjustment
convention). Range-scoped by construction: in/out is the clip's own span.
Mechanism is the §4 sub-composite: below-stack into the accumulator,
adjustment stack applied, compositing continues. Rejected: track-level
effect stacks — they cannot be ranged, feathered in time, or keyframed
per shot without reinventing clips. Cost: one enum variant, the
accumulator pass, plus keep-outside survival rules for the new kind.

**Compound clips (nests).** New `ClipContent::Compound` referencing a
named nested `Sequence` stored on `Document` beside the main timeline,
with an explicit addressing model: IDs unique across sequences; one
clip/track resolver landed first as a pure refactor; nested sequences map
via `map_frames` at their own fps, like assets; compound audio is a
mixdown of the nested `TrackMix`es (Riel ruled); clip effects see the
precomposed children (the Premiere nest convention). Sequences are
editable timelines, so nesting is reuse, not flattening; nest/unnest
ships in the same slice. Rejected: inline nested clip vectors (no reuse,
ID instability, journal bloat) and muted-v1 audio (a nest that drops
sound lies). Cost: the programme's largest core-model item — resolver,
`Sequence` table, cycle validation, nested recursion with a depth cap,
mixdown, step in/out UI — in its own slice (MO3) behind a call-site
probe.

**Blend modes.** Per-clip typed `blend_mode` (a `Clip` field, not an
effect parameter — blending happens between composited layers, not inside
a layer's param fold): `Normal` plus an editorial six (`Multiply`,
`Screen`, `Overlay`, `Darken`, `Lighten`, `Add`), evaluated in
scene-linear through the sub-composite pass, with `Normal` exactly
today's alpha-over. Rejected: the full AE mode set and per-effect
blending. Cost: one enum, the accumulator branch set, golden renders per
mode on both lanes, and the CPU-reference twin.

**Transform completion.** Extend `transform` with per-axis scale
(`scale_x_percent` / `scale_y_percent`, neutral 100, multiplying the
uniform master), `rotation_centidegrees`, and `anchor_x` / `anchor_y` in
basis points of layer size — all keyframe-able under §3. Rejected: a
second `transform2` effect (fragments every consumer of the motion
surface). Cost: uniform-block growth, vertex math in WGSL, golden renders;
existing projects resolve missing params to neutral unchanged.

**Masks and mattes vs CC5 node mattes.** The hard boundary stays: CC5
node mattes scope colour corrections and never touch alpha; layer masks
and track mattes shape alpha only. MO4 extends the `mask` effect to
multi-window (up to four aspect-corrected windows, union/intersection,
per-window invert — the CC5 window vocabulary reused, not reinvented),
adds track mattes in the Premiere keying convention (`Clip` nominates a
*track* whose composite below is its matte source — never a clip ID, which
would dangle after splits and admit cycles — with alpha/luma channel and
optional invert), and polishes `chroma_key` with tracker-driven animation
via the existing `track_region` engine. Rejected: merging node mattes and
layer masks into one system — they evaluate at different stages (inside
the grade stack vs at composite) and merging would tangle CC1's
no-intermediate-clamp invariant. Person/segmentation mattes are a
deferred intended gap (§11): when they arrive, the matte must come from a
real model whose polarity is verified on a rendered frame — hand-drawn
approximations are not accepted as person mattes. Cost: mask-window
evaluation in the shader, one matte resolve pass, tracker planners
mirroring `track_matte_window`.

**Still images.** New `MediaKind::Image`: probed stills (PNG/JPEG via the
existing FFmpeg decode path) with a one-frame source duration; a still
clip holds that frame across its project-local span, exactly the freeze
render shape (`source_at`/`source_end` one frame apart) with full trim,
effects, and animation. Rejected: modelling stills as one-frame video
assets with no kind — probe honesty (duration/fps claims) and relink
fingerprinting both need the kind to be explicit. Cost: probe/decode arms,
one render path shared with freeze, Ken Burns gates in MO1/MO7. Stills
land in MO1 so the first slice ends with the Ken Burns demo.

**Solids.** `ClipContent::Solid` (integer RGB) lands in MO2 as matte and
adjustment fixture material — a trivial fill render, no decode — and is
reused by MO6 templates for backgrounds and bars. Rejected: shipping
solids only with templates (MO2's accumulator and matte gates need a
cheaper fixture than decoded media). Cost: one enum variant, one fill arm.

**Enable flags.** MO1 adds a per-effect `enabled` flag and a clip enable
toggle (§3): disabled effects are skipped in `evaluated_at`/`params_for`,
disabled clips are skipped in layer resolution. Both are keyframe-able
(`Hold` for cuts) and revision-gated like every mutation. Rejected: a
bypass implemented by removing the effect (destroys values and keys).
Cost: two flags, skip arms, toggle UI.

## 5. Retiming

**Recommendation: time remap is an `AutomationCurve` on the clip.**
New `Clip.time_remap: Option<AutomationCurve>`, keyed in clip-local
project frames, valued in integer milli-frames of source position. When
present it replaces `speed_percent`, which becomes the parked value (the
AU4 `audio_gain_tenth_db` pattern: one scalar, one curve, never both
live). Flat segments are freeze holds, negative slopes are reverse, slopes
are ramps — one representation covers ramps, reverse, freeze, and
hold-or-hide; loop stays deferred. Remap keys are kept outside in/out
under §3 (a trim must never change which source frame shows).
Out-of-range evaluation follows an explicit per-clip policy token, `Hold`
(edge frame, default) or `Hide` (transparent), instead of failing; past
the last key the curve clamps (a hold), it never invents motion.

**The mapping is a `ClipTimeMap`, and the compiler enforces it.**
`clip_effective_fps` is replaced by an enum — `Constant(Rational)` |
`Remap` — so every call site must handle remapped clips to compile:
duration, split/trim boundaries, decode positioning, agent durations.
For remapped clips the project duration is the stored trimmed extent
(it cannot derive from `source_range`, which instead denotes the
accessible source window the curve is clamped to); evaluation goes
through one pure integer kernel over the key slice (a Kani candidate
after the §3 probe). Asset fps ≠ project fps converts at the boundary
via `map_frames`; linked A/V members take the remap together (video
remaps, audio muted), exactly as M23 speeds both. Rejected: keeping
`clip_effective_fps` with a remap side-channel (call sites would silently
do the wrong thing) and float-valued remap curves (breaks integer-exact
split adjacency).

**Frame blending: mix only; optical flow is a deferred Premiere gap.**
Per-clip sampling token: `Nearest` (today's behaviour, default) or
`Blend` (dissolve between the two adjacent source frames, decoded and
mixed in `FrameRenderer`). Optical flow ships nowhere here — it needs a
model, heavy compute, and a cache story — but it is honestly a Premiere
gap, not AE scope, and §11 records it as intended future work. Audio
under any remap stays muted (the M23 honest middle ground);
pitch-preserving stretch is DSP scope, not motion scope. Derived mappings
(transcript words, silence, scenes) skip remapped clips exactly as M23
skips speeded ones — recorded, not silently wrong.

**Rejected alternative.** A separate ramp object (speed-over-time plus a
freeze list plus a reverse flag): three representations needing three
survival policies, where one curve reuses survival, the rubber-band
pattern, and planner plumbing. The UI shows derived speed-%; the document
stores positions, never speeds.

**Cost.** One curve owner with keep-outside survival rules, the
`ClipTimeMap` replacement with one pure kernel (a Kani candidate),
two-frame decode+mix in the renderer, one rubber-band editor, one planner
(`plan_speed_ramp`). The export path must decode non-monotonically
(reverse-adjacent access) — the frame cache makes this a policy change,
not a rewrite.

## 6. Titles and graphics templates

**Recommendation.** MO6 ships three things, all vector or code-generated,
no raster:

1. **Animated titles.** Title clips already carry `Clip.effects`, so whole-
   title motion (position, scale, rotation, opacity, crop) arrives with
   MO1/MO2. MO6 adds entrance/exit presets (fade, slide, whole-word
   typewriter reveal) as planner-authored keyframes on the existing
   effects — plus one new integer `Title` field (words revealed) with an
   `AutomationCurve` owner for the typewriter. Title fade lengths stay
   what they are, durations, not keyframe-able fields. Per-character
   animators and path text are AE-programme scope.
2. **Templates reuse the nested `Sequence`.** A `GraphicTemplate` is a
   named sequence (titles, solids, adjustment-backed styling) with named
   typed editable fields (text, colour/font tokens, durations, positions);
   instantiating it nests that sequence as a compound clip with bound
   field values — no second stored-clip-set machinery. Built-ins are
   deterministic generated assets (the CC4 precedent, hash-pinned); any
   title/compound selection can be saved as a user template; effect and
   motion presets are the same mechanism at fragment scale and ship in
   the same slice.
3. **Solids reused.** `ClipContent::Solid` lands in MO2 (§4); MO6 spends
   nothing new on primitives.

Template fields are the bindable surface a future creator-packages slice
can generate role-scoped edit panels against; the generated-panel UI
itself is creator scope, not MO6.

**Rejected alternatives.** A second clip-set table beside `Sequence`;
SVG import and pen/path tools (AE scope); a sidecar template store
(small JSON belongs in-document with journal/undo); per-character text
animators (whole-word reveal covers the editorial jobs).

**Cost.** Field bindings plus instantiate/save operations, the typewriter
field and owner, a built-in set with pinned renders (lower third, title
card, end card, two caption styles), the entrance/exit planners, and the
preset mechanism. No new renderer: solids fill, titles reuse
`title_layout`, styling reuses MO2/MO5.

## 7. Agent surface and efficiency

**Recommendation: registry-only growth, served quad frozen.** Every new
operation (single-key upsert/remove, `ClipTimeMap` setters, blend-mode
and matte setters, adjustment/compound/solid/template/preset operations,
widened still import, enable toggles, nest/unnest) arrives as a generated
mutator reached through `invoke_capability`, never as a served tool.
Every new planner and inspector is likewise registry-only. The served
quad stays pinned at its nineteenth measurement (7 tools / 5,660 B /
3,510 B / 998 B); each slice re-measures and asserts byte-identity in the
existing pin sites, and records registry growth in the M36 ledger exactly
as AU4/AU5 did. New descriptor rows (transform extension, mask windows)
get compact pattern docs in tool descriptions (the
`color_curves`/parametric-EQ precedent), and planner descriptions keep
the 1,024 B budget with the load-bearing clause in sentence one (the
`get_capability` first-sentence rule).

**Token budgets bound the key emitters.** Tracker and remap planners can
emit per-frame keys, so each slice brief pins a byte budget per planner
response; planners decimate within a pinned value tolerance and return
compact curve summaries (span, key count, extrema), full keys on request.
Over budget fails closed with a summary, never a silently decimated
curve. Rejected: unbounded per-frame emission and lossy-by-default
decimation.

**Perception: one solo-preview capability.** `preview_solo` is a
registry capability (not a served tool) that renders one clip, compound,
or adjustment over its active window and returns the strip as an image
(`ContentBlock::image`, as storyboards do) plus a sampling-report JSON, reusing the proof render path with
disciplined defaults (8–16 samples, same-sample before/after, full-res
fallback for small text). Soloing an adjustment clip returns a
before/after pair — the only honest rendering of an effect whose input is
the composite below. Motion planners are evidence-only and revision-gated
like their colour/audio siblings.

**Parity: every capability has a GUI path.** Keyframe editor with
opacity rubber band (MO1), preview-overlay direct manipulation for
transform/mask (the CC5 overlay precedent, MO2/MO5), rubber-band retime
editor (MO4), nest step in/out (MO3), template field panel (MO6). Per the
investigator programme's "power available, expertise not required":
planners propose typed operations, the person answers judgement questions
only, and no recovery question is ever asked — typed incidents with
policy-class recoveries cover the new failure modes (compound reference
breaks, remap range policy, tracker low-confidence).

**Transparent export: specified, not built.** The programme adds no
export lane. MO6's design specifies the isolated-layer transparent
export shape (one layer over its active window, placement preserved,
alpha preserved, no audio, verified by alpha-extract plus
dual-background composite) as the contract the bounded codec lane
implements later.

**Rejected alternative.** Serving motion tools on MCP directly (IN1
measured +35.5% served bytes for just two tools — the split surface
exists precisely to forbid this) and whole-schema dumps for the new
descriptors (the M36 posture forbids it; pattern docs instead).

**Cost.** Roughly fifteen generated operations and half a dozen
capabilities across the programme, all registry-side; one perception
capability; per-slice ledger rows and byte budgets. No served byte may
move.

## 8. Slices MO1–MO7

The recommended order is validated with these adjustments: MO1 absorbs
transform completion, stills (Ken Burns demo), the enable toggles,
copy/paste attributes, and the opacity rubber band; MO2 is split (blend
+ adjustment + transitions + solids + solo; compound moves out); compound
is its own slice (MO3) behind a call-site probe; templates reuse the
nested `Sequence` (MO6). Gates below are named behaviours, not test
counts — preview/export golden-identity proves little (one shared
`FrameRenderer::render`), so survival, differential, and lane-named
renders carry the weight. Every render gate names its lane: lavapipe GPU
renders and/or CPU-reference differentials.

**MO1 — Keyframes, transform, stills, enable.** Single-key upsert/remove
operations; transform completion (§4); keep-outside survival for video
owners (§3); per-effect `enabled` + clip enable; copy/paste attributes;
the inspector keyframe editor with timeline lanes and opacity rubber
band; scale-to-frame; `MediaKind::Image`; one motion planner; pure
kernels with a stage-0 Kani probe before any harness count is pinned.
*Exit gate:* a push-in survives trim-in-then-out; split copies keys to
both halves; still import → scale → Ken Burns renders (lavapipe);
tangents round-trip ignored; probe verdict recorded; served quad
unchanged. *Depends:* nothing. *Size:* extra large, two to three parts.

**MO2 — Blend, adjustment, transitions, solids, solo.** Per-clip blend
modes (Normal + six) through the sub-composite pass; `ClipContent::`
`Adjustment`; push/slide/wipe transitions; `ClipContent::Solid`;
`preview_solo` as an image-returning registry capability;
CPU-reference twins; fps floor + VRAM ceiling pinned. *Exit gate:* Normal
bit-identical to pre-MO2 on both lanes; each other mode matches its CPU
twin (lavapipe + CPU reference); an adjustment over two tracks equals the
hand-computed stack; each new transition resolves its midpoint frame;
adjustment solo returns a before/after pair; fps/VRAM inside budget;
served quad unchanged. *Depends:* MO1. *Size:* large, two parts.

**MO3 — Compound clips.** Call-site probe first (measures the resolver
blast radius); then the resolver refactor as a pure behaviour-preserving
change; the `Sequence` table with cross-sequence unique IDs and cycle
validation; nested `map_frames` at sequence fps; audio mixdown;
nest/unnest with step in/out UI. *Exit gate:* probe numbers size the
brief; the refactor changes no golden byte; a nest renders identically
to its flat equivalent (both lanes); cycle/self-nest rejected; mixdown
nulls against the flat mix; served quad unchanged. *Depends:* MO1, MO2.
*Size:* large, two parts.

**MO4 — Speed ramps and time remap.** `ClipTimeMap` replacing
`clip_effective_fps` at every call site; `Clip.time_remap` with
keep-outside survival; `Nearest`/`Blend` sampling; rubber-band retime
editor; `plan_speed_ramp` under a byte budget with decimation; audio-muted
and derived-skip honesty. *Exit gate:* a ramp holds its endpoints across a trim; reverse renders
the mirrored source walk (lavapipe); Hold-vs-Hide differ exactly at the
boundary frame; blend-vs-nearest differs by a pinned pixel count;
planner response inside budget; served quad unchanged. *Depends:* MO1.
*Size:* medium-large, two parts.

**MO5 — Mattes and keys.** Multi-window `mask`, track-nominated mattes
(alpha/luma + invert), `chroma_key` polish, tracker-driven mask planner
on `track_region` with decimation and low-confidence refusal. *Exit gate:*
a 4-window matte contains exactly its windows (both lanes); alpha and
luma track mattes match hand-built references; a tracked mask holds
containment with pinned occlusion behaviour; low confidence refuses;
served quad unchanged. *Depends:* MO1, MO2. *Size:* medium, two parts.

**MO6 — Titles and graphics templates.** Typewriter field + owner,
entrance/exit preset planners, templates-as-sequences with field
bindings, effect/motion presets, ≥5 built-ins with pinned renders, the
isolated-layer transparent-export shape specified for the codec lane.
*Exit gate:* a lower third animates through preview and export (both
lanes); template save/instantiate round-trips bound; a preset applies
its keys verbatim; all built-ins match pinned renders; served quad
unchanged. *Depends:* MO1–MO5. *Size:* medium, two parts.

**MO7 — Workflow evaluation.** Scenario authority (`mo_scenarios`) with
synthetic lossless sources; technical gates as ordinary `cargo test` on
both CI operating systems; scripted agent and person paths; a new
`motion-workflow` eval suite with blinded review; the IN3 post-render
cut-boundary self-check run over every motion scenario output (flash at a
cut, overlay hiding a caption, audio pop, level jump); a publishable-output
critic pass that reviews the render adversarially with timecoded evidence
and a ranked fix list; no new feature, no new tool. *Exit gate:* all scenario gates green on both OSes with lanes
named; every workflow completable through the real endpoint and through
the GUI; token/tool-call budgets green; human reviewer left only
creative questions. *Depends:* MO1–MO6. *Size:* medium, one part plus
the real-harness run and blind review.

## 9. Evaluation matrix

| Editor job | Verified by | Human question, if any |
| --- | --- | --- |
| Push-in / pull-out on a shot | Keyframe contract tests, Kani `value_at`/`ease` proofs, preview/export golden renders, hands-on | Does the move feel intentional, not mechanical? |
| Ken Burns over a still | Still-import tests, hold-render parity, hands-on | None (technical) |
| Picture-in-picture with blend | 7 golden blend renders, hands-on | Does the composite sit believably? |
| Scene-wide look via adjustment layer | Below-stack render tests, hands-on | Does the look hold across the cuts? |
| Nested sequence edit (compound) | Cycle/self-nest rejection tests, precomp render, hands-on | None (technical) |
| Speed ramp into a freeze hold | Remap contract tests, Kani `remap_source_at` proofs, blend-vs-nearest pinned diff, hands-on | Does the ramp land on the beat/moment? |
| Reverse shot | Negative-slope contract tests, render, hands-on | None (technical) |
| Masked isolation / track matte | Window-containment renders, alpha/luma matte renders, tracked-mask sequence with occlusion, hands-on | Are any matte edges or tracking corrections visible? |
| Animated lower third from a template | Template round-trip tests, 5 pinned built-in renders, hands-on | Does the animation suit the story? |
| Agent-authored motion end to end | Scripted real-endpoint tests (plan → commit → proof), token/tool-call budgets, hands-on | Only the creative choices, never the mechanics |
| Encoded delivery of a motion-heavy timeline | Tags, decoded pixel comparison, platform consistency | Only if a codec limitation creates a visible trade-off |

Every slice brief pins numeric thresholds before implementation (pixel
diffs, containment percentages, budget bytes, planner byte budgets) and
names the render lane per gate (lavapipe, CPU reference, or both) —
"within tolerance" is not a gate until the number, method, and lane
exist. The MO7 rubrics state our own anti-inflation rule in three gates:
a result must pass structural (it saves, resolves, renders), technical
(frames, matte, audio, dims, codec measure right), and creative (story,
pacing, taste) review in order, and passing the first two is never
grounds for calling the result excellent.

## 10. Risks and open questions for Riel

Risks:

- **MO1 and MO3 are the biggest slices.** MO1 carries the editor,
  survival, stills, and the Kani probe; MO3 carries the resolver refactor
  (~1000 `document.tracks` call sites — the probe sizes this first). If
  either runs hot, split it rather than thinning it.
- **GPU downlevel limits.** CC3 already hit storage-binding limits; the
  accumulator plus uniform growth must fit the same backends inside the
  MO2 fps/VRAM budget, or the design budgets further passes.
- **Tracker ceiling.** `track_region` is normalized-SAD template matching;
  CC7 pins a one-sample occlusion, but mask tracking through long
  occlusions will drift — MO5 must fail closed with confidence refusals,
  not silently bad mattes.
- **IN3 dependency.** Motion adds roughly fifteen operations and half a
  dozen capabilities to the registry; if task-scoped capability packs
  slip, planner descriptions and byte budgets must be extra lean to hold
  token budgets.

Decided since revision 1 (Riel's answers + N1, recorded here so the
questions are closed, not dropped): keys are kept outside in/out for
video and remap curves; compound clips are full editable nested sequences
with audio mixdown; blend is evaluated in scene-linear; expressions are
deferred to the AE programme; optical flow, Bezier handles, motion blur,
and person/segmentation mattes are deferred but intended (§11).

Open questions, defaulted by the lead on 2026-09-23 (Riel may override):

1. **Blend set:** Normal + six for the first cut; further modes are future work.
2. **Wipe subset:** straight push/slide/wipes in MO2; clock, iris and gradient-wipe
   variants are future work.

## 11. Future work (deferred but intended)

Riel wants all of these eventually; each has an owner, not a rejection:

- **Optical-flow retiming.** Owner: a future retiming-depth slice (needs
  a model choice, compute budget, and a cache story).
- **Bezier keyframe handles.** Owner: the AE programme; tangent fields
  reserved on `Keyframe` in MO1 for a clean migration.
- **Motion blur.** Shutter-accurate blur. Owner: the AE programme; no
  approximation ships in MO.
- **Person/segmentation mattes.** Owner: a future ML-matte slice (needs
  a model dependency and license decision); real models only, polarity
  verified on a rendered frame.
- **Typed parameter expressions.** Owner: the AE programme, with a GUI
  path as a condition of the design.
