# MO1 — Keyframes, transform, stills, enable

> Status: **promoted 2026-09-24** — revision 2 after one Opus critique (`revise`) and the
> stage-0 Kani probe; all findings accepted with the critic's fixes.
> Programme `MO0-MOTION-PROGRAMME.md` (promoted rev 2) · Recipe v2

Basis: MO0 §3 (one curve type, keep-outside survival, reserved tangents, single-key
upsert/remove, enable flags), MO0 §4 (transform completion, stills, enable), MO0 §7
(registry-only growth, served quad), MO0 §8 (the MO1 row), MO0 §11 (future work); the
roadmap "Motion and compositing programme" section; CC3 §6 (keyframing policy); AU4
(envelope/survival/planner precedent); the stage-0 Kani probe report (DONE 2026-09-24
— verdict recorded in §8). Conventions per MO0 §0: prose cites symbols, never
`file:line`; exhaustive inventories live in compile-forced tests. Rules `R1..` are
numbered and testable; each names its pin.

## Changes in revision 2

- B1 → R13 (ordered-only validation, negative `at` allowed), R15/R16/R18 routing,
  R23 (negative keys clipped from lanes).
- B2 → R2/R3 (six `LayerParams` fields, aspect-corrected rotation), R10 (canonical
  bake into `scale_x/scale_y` + fine), R26 (L-shaped 16:9 golden).
- B3 → R1 (fine triple `x/y_basis_points`, `scale_fine_hundredths` + canonical-writer
  rule), R2 (fold, zero new lane fields), §10 gate 11 (sub-pixel push-in).
- B4 → §8 rewritten to the probe verdict (R31: i64 kernel IS `value_at`; R32: H1–H4
  pinned N≤4 i64 ±1e6 + H6, seam-H5 dropped, CI cost + solver pin); §10 gate 12.
- S1 → R31 (hold-first, checked-`None` fallback, descriptor sweep test); the MO4
  milli-frame flag → §11.
- S2 → §3/R13 (colour nodes keep-outside); R19 (copy onto shorter clips legal).
- S3 → R4 (sibling `Effect.enabled_curve`, `EffectWire`, `DisabledEffectOnAudioChain`,
  every-reader skip); R15/R17 routing nits.
- S4 → R7/R8/R9 (stills are `Freeze{source_frame: 0}` over `Image`; zero new
  variants; `add_clip` reuse; span-content relaxation; still op list; still risks).
- S5 → R12 (three-point/replace/fill rows); §10 gate 1 scoped to video owners; R17
  (linked A/V enable rule).
- S6 → §9 (literal passes counted in Part A; skip arms moved to Part B).
- S7 → §10 (gate 11 → pin; gate 2 eased + key count; gates 5/6 GPU-free primary).
- S8 → R22 (auto-key, prev/next nav, reset), R16 (last-key-to-static), R23 (dimmed
  disabled clips); three MO2 deferrals → §11.
- Nits → R17 (`AddEffect` carries disabled), R19 ((name, occurrence) + order), R20
  (`SetEffectKeyframes` emission; `ken_burns` on video allowed), R21 (sextuple),
  R4/R5 (non-Hold step documented).

## 0. Goal

Give every clip keyframed motion (completed transform, opacity, crop), still-image media
with a Ken Burns path, and per-effect plus clip enable toggles — by hand and by agent —
on the existing `AutomationCurve` model with keep-outside video survival.

## 1. Scope

### 1.1 In scope

Single-key upsert/remove operations; keep-outside survival for video and colour-node
owners; transform completion (per-axis scale, rotation, anchor, fine triple); per-effect
`enabled` plus clip enable, both keyframe-able; reserved tangent fields on `Keyframe`;
`MediaKind::Image` stills as freeze clips with scale-to-frame; copy/paste attributes;
the inspector keyframe editor (auto-keying) with timeline key lanes and an opacity
rubber band; one motion planner (`plan_motion`); slice-level i64 kernels with Kani
harnesses (H1–H4 pinned at N≤4 i64 ±1e6, H6 measured in Part A).

### 1.2 Out of scope

- MO2: blend modes, adjustment clips, push/slide/wipe transitions, solids,
  `preview_solo`, the CPU-reference twin, the fps/VRAM budget.
- MO3: compound clips and the resolver refactor. MO4: `ClipTimeMap`, `time_remap`,
  frame blending, the retime editor.
- MO5: multi-window masks, track mattes, `chroma_key` polish, tracker-driven planners.
- MO6: title entrance/exit presets, templates-as-sequences, effect/motion presets,
  the transparent-export shape. MO7: scenario evaluation.
- Future work (MO0 §11, all owned elsewhere): Bezier evaluation and handles (tangent
  fields are reserved here, never evaluated), motion blur (no approximation ships),
  optical-flow retiming, person/segmentation mattes, typed parameter expressions.
- Timeline key-lane drag-move: deferred to the MO2 overlay-manipulation work (§11).

## 2. Data model

**R1 (transform completion).** The `transform` descriptor gains eight parameters, all
keyframe-able under the existing `Effect.keyframes` machinery: `scale_x_percent` and
`scale_y_percent` (1..400, neutral 100, each multiplying the uniform `scale_percent`
master), `rotation_centidegrees` (-36000..36000, neutral 0 — one full turn each way;
wider spins are a descriptor-only widening later), `anchor_x_basis_points` and
`anchor_y_basis_points` (0..10000, neutral 5000 — centre; off-layer pivots deferred),
plus the fine triple — `x_basis_points` and `y_basis_points` (-10000..10000, neutral
0, basis points of frame, additive with `x/y_percent`) and `scale_fine_hundredths`
(100..40000, neutral 10000 = ×1.0, multiplying with master and axes). Whole-percent
params step visibly (1% ≈ 19 px at 1080p; B3), so planners and GUI drags follow the
canonical-writer rule: hold the coarse param constant over a move and ramp the fine
one (fractions live in fine); readers sum position / multiply scale, so the
redundancy is accepted and deterministic. Pinned by the descriptor-table fixture and
a canonical-form test. Rejected: a second `transform2` effect (MO0 §4 — it would
fragment the motion surface).

**R2 (transform fold).** `EffectUniform` gains eight variants (`ScaleX`, `ScaleY`,
`Rotation`, `AnchorX`, `AnchorY`, `ScaleFine`, `OffsetXBasisPoints`,
`OffsetYBasisPoints`); `LayerParams` gains six fields: `scale_x`, `scale_y`,
`rotation` (radians), `frame_aspect` (frame height ÷ width), `anchor_x`, `anchor_y`
(fractions). `params_for` folds: effective axis scale = master/100 × axis/100 ×
fine/10000; NDC offset = coarse/50 + basis-points/5000 (the existing coarse/50 arm
preserved exactly); rotation = centidegrees × π/18000; anchor = basis-points/10000.
The B3 fine params fold into the existing lanes — zero new `LayerParams` fields for
them. Absent parameters resolve to descriptor neutral, so existing projects render
unchanged. Pinned by fold unit tests plus lavapipe goldens (§6).

**R3 (composite order).** The vertex transform applies translate(-anchor), then
per-axis scale, then aspect-corrected rotation about the anchor, then
translate(anchor), then the offset. Rotation is corrected for output aspect (B2 —
NDC rotation shears on non-square frames): with `a` = `frame_aspect`, correct
(x, y) → (x, y×a), rotate, uncorrect (rx, ry) → (rx, ry/a). Positive rotation reads
clockwise on screen (the Premiere convention); rotation 0 is exactly the pre-MO1
path (correction cancels). Pinned by an L-shaped golden on 16:9, which
distinguishes every wrong order, sign, and shear.

**R4 (per-effect `enabled`).** `Effect` gains `enabled: bool` (default true, skipped
when true — no stored project changes a byte) and a sibling `enabled_curve:
Option<AutomationCurve>` — deliberately NOT a `Effect.keyframes` entry, so the ~36
loops over `effect.keyframes` that assume registered parameter names are untouched
(S3). Validation accepts curve values 0..1 under every interpolation; non-`Hold`
kinds act as a step (the ≥ 1 test resolves every frame, mid-ramp frames
deterministically — documented, the `bypass` precedent from CC3 §6, not refused).
Resolution is a new `Effect::is_enabled_at(at)`: curve `value_at` ≥ 1, else the
static flag. Single-key and whole-curve operations address the sibling through the
uniform name `"enabled"` (R15/R16 and `Set`/`ClearEffectKeyframes` route it;
storage stays the sibling). The hand-written `EffectWire` deserializer gains both
fields with defaults. Bus/master audio effects reject `enabled: false` or a curve
in their validators with a new `OpError::DisabledEffectOnAudioChain` — MO1's
simplest honest rule (honouring it in `chain_structure_matches` and live retune is
future work; rejected alternative recorded). Skip: EVERY `Effect` reader filters
via `is_enabled_at` — `evaluated_effects`, `params_for`, the engine matte path,
`color_qc/nodes.rs`, the agent `server.rs` render, and the app inspector
fingerprint plus `preview_ui`. Pinned by serde-identity, wire-default, routing,
rejection, and per-reader skip tests.

**R5 (clip enable).** `Clip` gains `enabled: bool` (default true, skipped when true)
and `enabled_curve: Option<AutomationCurve>` (values 0..1, enabled iff value ≥ 1,
non-`Hold` kinds act as a step exactly as R4). A disabled clip is skipped in
`visual_layers_at`, `video_layers_at`, `timeline_source_at`, and
`timeline_audio_segments` — silence included. `enabled_curve` follows keep-outside
survival (§3). Pinned by layer/audio resolution tests.

**R6 (reserved tangents).** `Keyframe` gains `tangent_in: i64` and `tangent_out: i64`,
serde-defaulted to 0 and skipped when 0: existing documents are byte-identical, and
no `KeyframeInterpolation` variant is added, so every existing match stays
exhaustive. Evaluation ignores both fields for every interpolation kind; validation
accepts any value (a future writer's tangents must not break this reader); survival
preserves them verbatim (they ride the existing `..*key` updates, and the §8 kernels
carry them too). Pinned by a round-trip test with nonzero tangents and an
evaluation-identity test (nonzero tangents change no `value_at`).

**R7 (`MediaKind::Image`).** A fourth `MediaKind` variant for probed stills (PNG/JPEG
via the existing FFmpeg decode path). `supports` admits `Image` on video tracks only.
`probe_path` assigns `Image` when the best video stream uses a still-image codec and
packet analysis finds exactly one frame; fps is `Rational::default()` (nominal —
no mapping may depend on it, and under R8 none does), duration is one frame,
resolution is the decoded dimensions, colour description comes from the decoder
exactly as for video. Named still risks (S4): EXIF orientation is applied at decode
(pinned by an oriented fixture); PNG alpha is preserved through the hold path into
alpha-over; stills over 8192 px on either axis are refused at probe with a typed
`MediaError::Backend` (named limit, tested — downscale-on-import is future work,
§11); untagged PNGs carry `ColorDescription::unknown` into the existing
source-colour incident path (no new incident). Pinned by probe fixtures (still vs
video vs audio) and the kind-exhaustiveness tests every new variant forces.

**R8 (stills are freeze clips).** A still is `ClipContent::Freeze` with
`source_frame: 0` over a `MediaKind::Image` asset — no new content kind, no
`clip_duration`/`split_clip`/`trim_clip` media-mapping changes (the rev-1
Media-span special case is deleted; S4's 24 fps objection resolved by
construction). Still import uses `AddFreezeFrame` (5 s span, `source_frame: 0`,
which passes `validate_freeze_source_frame` against the one-frame asset); the
existing freeze paths supply span duration, the hold render in `visual_layers_at`,
audio exclusion, `FreezeClipHasNoAudio`, and `SpeedOnNonMediaClip`. Full trim,
effects, and animation apply. Relink requires kind equality (`Image`↔`Image`) or
the existing `RelinkMetadataMismatch`. Derived mappings (transcript, silence,
scenes) skip `Image` clips, recorded exactly as M23 skips speeded ones. Pinned by
import, duration, trim, relink, and render-hold tests.

**R9 (still/content integrity).** `add_clip` rejects `Image` assets with the existing
`InvalidSourceRange` (a Media range over a still is invalid — stills enter via
`AddFreezeFrame`); the stills design mints zero new `OpError` variants.
`require_media` relaxes to span-or-media: Media, Title, and Freeze clips all
participate in `roll_edit`/`slide_clip` with project-frame arithmetic (spans have no
source to slip — slide moves the span window, roll moves the shared edit point;
a behaviour change for Title neighbours, with a CHANGELOG line). Each op's still
behaviour: add/trim/split/slide/roll ride the freeze paths; speed and audio fail on
the existing Freeze arms; `ReplaceClip`/`FitToFill` still require Media
(`EditorialRequiresMedia` — replace a still by delete+add); ripple moves stills
with their tracks; three-point edits split them like any clip. Pinned by rejection,
relaxation-combination, and per-op still tests.

**R10 (scale-to-frame).** Every layer is a full-frame quad with the source stretched
to fill (B2 — a uniform shrink cannot un-stretch), so still import bakes the fit
into `scale_x_percent`/`scale_y_percent`: with r = (img_w × frame_h) ÷ (img_h ×
frame_w) as an exact rational, the limiting axis takes 100 and the other takes
100 × min(r, 1/r) — 100 on the limiting axis, aspect on the other. Canonical form:
the coarse axis takes the floor percent (≥ 1) and `scale_fine_hundredths` takes the
remainder rounded to 0.01% (sub-pixel exact); the uniform master stays 100 and Ken
Burns ramps it. Baked, not render-time (Premiere's "Scale to Frame Size"): values
are inspectable, render stays a pure function of document state, and the rev-1
400-clamp problem vanishes (fractions, not magnification). Pinned by import tests
over portrait/landscape/tiny fixtures asserting displayed aspect equals image
aspect and fit-inside-frame.

## 3. Key survival rules

Video and colour-node `Effect.keyframes` (every effect where
`!is_audio_effect(name)`), `Effect.enabled_curve`, and `Clip.enabled_curve` keep
out-of-range keys (Riel ruled, Premiere behaviour — MO0 §3; S2 extends the ruling
to colour nodes, which also keeps `CopyClipAttributes` onto shorter clips legal).
Audio owners — `Clip.audio_gain_curve`, track/bus/master curves, and audio-effect
curves including bus/master — keep the AU4 rules verbatim.

**R11 (keep-outside kernel).** New `rebase_clip_curve_keep_outside(curve,
delta_local)`: shift every key by `-delta_local` (signed, saturating), drop nothing,
insert no boundary key. `rebase_clip_automation` applies it to keep-outside owners
and the existing `rebase_clip_curve` to audio owners. `delta_local` keeps its AU4
meaning (`new_timeline_start - old_timeline_start`, signed). Pinned by property
tests (shift identity, key-count preservation) and the §8 H4/H6 harnesses.

**R12 (per-operation policy).** Every operation that rewrites clip extents follows
R11 for keep-outside owners; audio owners keep AU4 §2.4 exactly:

- `TrimClip`: shift-only; trimming in then out restores the curve byte-identically.
- `SplitClip`: both halves receive the full curve shifted to their local origin
  (left `delta_local` zero, right `delta_local` the split offset) — the existing
  two call sites (`rebase_clip_automation` on the right half, `survive_clip_edit`
  on the left) both route through R11.
- `RollEdit`, `SlideClip`: shift-only on both touched clips with their signed
  deltas (AU4 R34 formulas unchanged).
- `SetClipSpeed`: keep-outside curves untouched (`delta_local` zero plus no drop
  equals identity — animation stays put in timeline space while footage scales
  under it); the audio envelope keeps its AU4 destructive-on-increase behaviour
  (AU4 R42), including its CHANGELOG line.
- `RelinkAsset`: shift-only with `delta_local` zero (identity for keep-outside);
  the row stays though currently unreachable (AU4 E4).
- `ThreePointEdit` / `PatchedThreePointEdit`: covered by composition — `split_clip`
  halves route through R11, and `clear_track_range` deletes clips wholesale (keys
  deleted with their clips, no survival rewrite).
- `ReplaceClip` / `FitToFill`: every clip curve kept verbatim, no rebase — the
  current behaviour, now pinned (new footage under the old move, like
  `SetClipSpeed`); legality follows the owner class on the next write.
- `SlipClip`, `MoveClip`, `DeleteClip`: no curve rewrite today (local origin
  unchanged, or the clip is gone) — unchanged.
- `RippleDeleteClip` / `RippleInsertGap`: clip-local curves untouched (local
  origins move with their clips); project-frame track/bus/master curves keep
  `ripple_delete_curve` / `ripple_insert_curve` and the `recompute_duration`
  `clamp_project_curve` pass, which never touches clip curves.

Pinned by one contract test per row above (keep-outside owner byte-exact, audio
owner AU4-verbatim), plus the trim-in-then-out round trip that is the slice's
headline gate (§10).

**R13 (ordered-only validation).** Keep-outside shifts go negative (head trim moves
key 0 to −20; B1), so keep-outside owners validate with a new
`AutomationCurve::validate_ordered` — non-empty and strictly ordered, sign-agnostic
— instead of `validate`. Negative `at` is legal on video/colour-node effect
curves, `Effect.enabled_curve`, and `Clip.enabled_curve`; `value_at` already
clamps, and GUI read-modify-write via `SetEffectKeyframes` on trimmed clips passes.
Audio owners keep strict `validate` (`NegativePosition` →
`InvalidEffectAutomation` as today), and `EffectKeyframeOutsideClip` is raised
only for audio-effect owners. The CC3 policies are untouched:
`is_hold_only_parameter` and `CurvePointCountAnimatedWithPoints` apply exactly as
today, including to keys written by the §4 single-key operations. Pinned by
accept/reject tests per owner class, including negative-`at` keep-outside curves.

**R14 (evaluation clamps).** `value_at` is unchanged: kept-outside keys past the
clip end or below zero simply never evaluate (`evaluated_effects` always resolves
at local frames ≥ 0), and trimming back out re-exposes them. No new evaluation
branch exists to disagree between preview, export, and agent inspection. Pinned by
the §8 H3 harness and a trim-cycle render test.

## 4. Operations, capabilities, and served-surface cost

All six operations below are `Operation` variants, hence revision-gated through the
existing Core actor / edit-plan path with no new gating machinery. Move-key-in-time
is remove+insert in one atomic batch (MO0 §3) — no dedicated operation.

**R15 (single-key upsert).** `UpsertEffectKeyframe { clip, effect, name, key }`
inserts `key` or replaces the key at `key.at`, keeping curve order. `name` routes
to the registered parameter or, for `"enabled"`, to the R4 sibling. It validates
the descriptor value range, hold-only legality (`NonHoldKeyframeParameter`), the
owner-class structural check (`validate_ordered` for keep-outside owners — negative
`at` legal — else strict `validate`), the R13 outside rule, and
`validate_curve_keyframe_policy` (upserting a second `point_count` key while
coordinates animate fails with `CurvePointCountAnimatedWithPoints`, and vice
versa). Naturally idempotent. Pinned by upsert/replace/order/policy tests.

**R16 (single-key remove).** `RemoveEffectKeyframe { clip, effect, name, at }`
removes the key at `at` (routing `"enabled"` to the R4 sibling, as R15). A missing
key is success with no change (idempotent — the agent retries safely); removing the
last key writes the removed key's value into the static parameter, then clears the
curve (S8 — the value survives as static, the `KEYFRAMED` badge clears). Pinned by
remove/missing/last-key-to-static tests.

**R17 (enable toggles).** `SetEffectEnabled { clip, effect, enabled }` writes the
static R4 flag; `SetClipEnabled { clip, enabled }` writes the static R5 flag.
`AddEffect` carries a full `Effect`, so disabled-at-creation is authored inline —
no follow-up toggle needed. Linked A/V: enable is per-clip; a linked pair is
enabled/disabled via an atomic batch of two `SetClipEnabled` (the `Clip.link`
precedent — core stays per-clip); the GUI toggle on a linked clip sends both, and
`plan_motion` never touches enable. Pinned by toggle/skip/linked-batch tests.

**R18 (clip enable curve).** `SetClipEnabledCurve { clip, curve }` with
`curve: Option<AutomationCurve>` follows the AU4 nullable-required pattern exactly
(`deserialize_required_curve` + `RequiredNullableCurve`, "null clears it; the field
is required", both halves pinned): omission fails, `null` clears. Values 0..1, any
interpolation, structural check `validate_ordered` (negatives legal), keep-outside
(no outside check). The GUI's "+ Key at playhead" reads-modifies-writes this whole
curve, exactly as the AU4 `ENVELOPE` block does. Pinned by wire, clear, and
survival tests.

**R19 (copy/paste attributes).** `CopyClipAttributes { from_clip, to_clip, names,
include_keyframes }`: `names: None` copies every effect, `Some` copies the named
subset; effects match by (name, occurrence) — the nth same-named source effect
targets the nth same-named target effect. A name unknown to the registry fails with
`UnknownEffect`; a name absent on the source is skipped (documented — the person
sees the result, the agent reads back). Each copied effect replaces its target
wholesale in place (values, `enabled`, `enabled_curve`, and `keyframes` iff
`include_keyframes`), keeping the target `EffectId` for stable references; names
absent on the target are appended after its existing effects in source order with
fresh ids. Copy onto shorter clips is legal (S2 — colour nodes keep outside).
One operation, one revision gate, one undo step. Pinned by verbatim-copy, subset,
skip, order, and id-stability tests.

**R20 (`plan_motion`).** One registry planner, revision-gated and evidence-only
(returns exact operations, applies nothing): `plan_motion { expected_revision,
clip_id, preset, replace }` with presets `push_in`, `pull_out`, `pan_left`,
`pan_right`, `ken_burns`, and `pip`. It emits whole-curve `SetEffectKeyframes` per
animated param (MO0 §3 — planners author whole curves; the fine triple carries the
ramps per the R1 canonical rule) plus `AddEffect`/`SetEffectParam` for static
`pip`; `ken_burns` is a move preset and is NOT refused on video. It refuses when
the target params already carry curves unless `replace: true` (AU4 R13 precedent),
emits at most 8 keys per parameter curve, answers within a 4 KiB response budget
(over budget fails closed with a summary, never a silently cut curve), and carries a
≤ 1,024 B description with the load-bearing clause in sentence one (the
`get_capability` first-sentence rule). Pinned by preset goldens, refusal tests, and
a budget assert.

**R21 (served quad frozen).** The six operations arrive as generated mutators and
`plan_motion` as a registry planner, all reached through `invoke_capability`; the
served quad stays byte-identical at 7 / 5,660 B / 3,510 B / 998 B for the twentieth
consecutive measurement, asserted in the existing pin sites. The registry sextuple
(generated/inspector/total counts + serialized/input/descriptions bytes) is
re-pinned with a per-part derivation: new transform descriptor rows cost
description bytes only (`Effect.parameters` is an untyped map — the AU2 Part A
precedent); the tangent, `enabled`, `enabled_curve`, and `Image` model fields grow
shared `$defs`, measured per embedding tool exactly as AU4 E20 did, with the
derivation in the ledger comment and M36 rows. Nothing pre-existing moves a byte.
Pinned by the sextuple pins plus the M36 ledger update in the same commit.

## 5. Person's GUI

Parity rule (MO0 §7): every §4 capability has a GUI path; planners propose typed
operations and the person answers judgement questions only.

**R22 (inspector keyframe editor).** A MOTION section on every clip inspector with
one card per motion effect (`transform`, `opacity`, `crop`, `reframe`, `mask`,
`chroma_key`): each parameter row reuses the generalised `keyframe_row` (AU4 R44/E51
— per-key frame/value/interpolation editing plus per-row delete), with an
interpolation picker over the five `KeyframeInterpolation` kinds, "+ Key at
playhead" (single-key upsert at the playhead frame), Clear (whole curve), and
prev/next-key navigation (jump the playhead across the card's keyed params).
Editing a keyframed param auto-keys (Premiere behaviour, S8): the edit upserts a
key at the playhead frame instead of writing a shadowed static. Static writes
happen only when the param carries no curve, plus explicit affordances: per-
parameter Reset (static to descriptor neutral, keys kept) and per-effect Reset (all
statics neutral + enabled on, keys kept). `transform` additionally gets numeric
rotation/anchor/fine editors. Each card carries an `enabled` toggle (R17); the clip
header carries the clip-enable toggle. A keyframed control shows the `KEYFRAMED`
badge. Drag gestures commit live and collapse to one undo step via the gesture-key
coalescing the CC1/CC3 sliders use. Pinned by widget-level tests on recorded rects
(the AU4 E55 precedent — the timeline/inspector have no pointer harness) plus a
one-undo-step test per gesture.

**R23 (timeline key lanes).** Video clips paint one key lane: a diamond per distinct
non-negative clip-local frame carrying any keep-outside effect key (union over
owners — owner identity lives in the inspector, not the lane; negative kept-outside
keys are clipped, reappearing on trim-out). Disabled clips render dimmed. Clicking
a diamond selects the clip and focuses the inspector's keyframe editor; bare
Delete/Backspace on a hovered diamond sends `RemoveEffectKeyframe` through the
one-shot hover-report arbitration AU4 E49 established (stale hover never swallows a
clip delete; `Shift+Del` still ripple-deletes; last-key delete performs R16
last-key-to-static). Lane drag-move, multi-key select/move, and key copy/paste are
deferred to MO2 (§11). Pinned by pure paint/hit helpers (`key_lane_diamonds`, hit
within the AU4 9 px radius) and the arbitration tests.

**R24 (opacity rubber band).** `opacity.percent` gets an AU4-envelope-style band on
video clips: one pure `opacity_band_rect` shared by paint, hit, and interact (AU4
R36); no band under 24 px of clip width; the interact registered after
`body`/`left`/`right` and intersected horizontally (last-registered-wins); parked-
value line plus click-to-insert-first-key when curve-free (AU4 E53, including its
honest click-cost note); drag writes through `envelope_coalesce_key`-style gesture
coalescing; vertical mapping linear 0..100 with the stored-value-on-floor rule (AU4
E58) so a horizontal drag never lifts a 0 key. One `show_envelopes` toggle governs
audio and opacity bands together. Pinned by rect/hit/drag math tests and a band-vs-
operation curve-identity test (a band gesture produces exactly the curve the
equivalent op path would).

**R25 (stills import and Ken Burns).** The file picker accepts stills; the media bin
badges `Image` assets; `add_asset_to_timeline` builds the R8/R10 clip
(`AddFreezeFrame`, 5 s span, canonical baked fit). The Ken Burns path by hand is
scale + position keys on that clip (§10 gate 3); by agent it is `plan_motion` with
`ken_burns`. Parity map (agent → GUI):
upsert/remove → keyframe editor, lanes, band; enable toggles → card/header toggles;
enable curve → "+ Key at playhead" whole-curve write; copy attributes → clip
context-menu copy/paste (same `CopyClipAttributes` op); `plan_motion` → "Apply"
confirmation previewing the exact operations. Pinned by an op↔GUI parity checklist
test (each §4 operation names its GUI sender) and the hands-on session (§12).

## 6. Render path

Preview, proof, and export share `FrameRenderer::render`; per MO0 §8,
preview/export golden-identity proves little, so the weight sits on contract tests,
differential pins, and lane-named renders.

**R26 (transform render).** `params_for` folds the R2 controls; the WGSL vertex
stage implements the R3 order. Float conversion (`params_for`, WGSL) is covered by
goldens, never Kani. Lanes: every R26 gate runs lavapipe; no CPU-reference twin
exists in MO1 — MO2 builds the twin together with the sub-composite accumulator,
and re-gates these goldens against it then. Pinned by: identity (transform-less
render byte-identical to pre-MO1), per-control goldens (scale master, per-axis,
rotation sign, anchor offset, coarse + fine position/scale — the fine goldens pin
sub-pixel response, e.g. a 5-basis-point nudge moves an edge exactly 1 px at
1080p), and the R3 L-shaped 16:9 order/shear golden.

**R27 (still render).** Still clips ride the existing freeze hold arm in
`visual_layers_at` (R8 — frame 0 held across the span, alpha preserved); the frame
cache pins the one still frame rather than churning it. Lanes: lavapipe for the
hold-parity gate (every frame of a 5 s still renders identical — hash the strip)
and the Ken Burns three-frame pins (first/mid/last match pinned hashes). Pinned by
hold-parity plus Ken Burns goldens.

**R28 (enable render).** Disabled effect ≡ effect absent (byte-identical render to
the clip with the effect removed); disabled clip ≡ clip absent from every layer and
silent in the mix (byte-identical to the clip removed). Keyframed `enabled` cuts at
the key's first frame (`Hold`-shaped step; the ≥ 1 test resolves mid-ramp frames
deterministically). Lanes: lavapipe for the visual halves, mix-null for the audio
half. Pinned by removal-identity tests.

**R29 (export honesty).** One motion-heavy timeline (push-in + Ken Burns + an
enable cut) exports through the delivery path and decodes back to frames matching
the proof renders within the established pixel tolerance method — proving the
shared path carries motion, not that two callers agree. Lane: lavapipe encode +
decode-compare. Pinned by the export round-trip test.

## 7. Incidents

**R30 (no new incident codes).** MO1 mints zero `IncidentCode`s: every new failure
maps through existing families, so incident growth stays output-only (the IN1 Part B
precedent). Exactly one new `OpError` variant exists: `DisabledEffectOnAudioChain`
(R4), joining the compile-forced (variant → `IncidentFamily`) table as `Malformed`
(beside `NonHoldKeyframeParameter`) and the (operation → `IncidentSubject`) table as
`Chain`; the six new operations join the subject table as `Clip` (and
`CopyClipAttributes` additionally names its source clip where the table shape
allows, else the target). Reused verbatim: `UnknownEffect`, `UnknownEffectParam`,
`InvalidEffectAutomation`, `InvalidSourceRange` (Media-on-`Image`, R9),
`EffectKeyframeOutsideClip` (audio-effect owners only, R13),
`NonHoldKeyframeParameter`, `CurvePointCountAnimatedWithPoints`,
`RelinkMetadataMismatch`, `EditorialRequiresMedia`, `FreezeClipHasNoAudio`,
`SpeedOnNonMediaClip`, `FitToFillUnrepresentable` (unchanged). Still-probe
failures surface as `MediaError::Backend` → `BackendUnclassified`; an undecodable
still format as `UnsupportedDecoderFormat` — no new media incident. Pinned by the
table tests plus one incident-render test for the new variant.

## 8. Pure kernels + Kani plan

The stage-0 probe is DONE (2026-09-24): i64 carries evaluation at N≤4 within the
< 5 min / < 6 GB admission rule, and no i128 cell fits at N=4. Production therefore
migrates to i64 (S1) so the shipped implementation IS the proved kernel — the
cross-width differential itself times out, so a proved shadow of an i128 production
path would leave prod/proof drift uncheckable.

**R31 (the i64 kernel).** `AutomationCurve::value_at` and `ease` migrate from
`i128` to `i64` intermediates, behaviour-preserving for validated documents (they
differ only if a key-value difference exceeds ~9.2e12 — unreachable, spans bounded
by document duration; the saturating first multiply is kept). The `Hold` arm
returns before any subtraction, so hold-only giants (`LUT_ASSET_ID_DESCRIPTOR_MAX`
2^53−1) never reach value arithmetic; outside the proven ±1e6 value range the
kernel uses checked arithmetic returning `None`, matching today's
`try_from().ok()`. The method bodies factor to slice-level free functions (the
proved units; methods are thin wrappers — equivalence by construction plus one
differential test); `ease` is proved through H1 with all five kinds symbolic; the
R11 shift body is the shift kernel. `Keyframe` tangents ride through untouched
(R6). A compile-forced sweep asserts every non-hold descriptor range plus the
clip-gain/track/bus/master curve ranges fit ±1e6. Pinned by the sweep, overflow-
`None` tests at the extremes, and the wrapper≡kernel differential.

**R32 (pinned harnesses).** Twelve harnesses, one `kani::assert` each, every shape
shown red against a deliberate mutation before green is trusted (probe log
carried): H1 (between-keys bounded, monotone kinds), H2 (exact-key value), H3
(clamp outside) at N = 2/3/4, i64, values ±1e6 — measured H1-N4 184 s / 0.9 GB,
H2-N4 152 s / 2.4 GB, H3-N4 55 s / 0.9 GB, all FIT; plus H4 (shift round-trip
identity) at N = 2/3/4 (≤ 0.5 s, values unbounded). New H6
(translation-equivariance: `value_at(shift(k,d), t−d) == value_at(k,t)`) is
measured in Part A and covers trim AND split — under keep-outside each split half
is a pure shift and value is preserved exactly. Seam-H5 is DROPPED: it measured
the AU4 drop+seam split, and AU4 rebase stays unproven unless it moves to a kernel
(explicit non-goal). Float pixel paths (§6) stay with differential fixtures, never
Kani. CI: the Kani suite runs as a separate CI job (not the per-commit gate), ~17
min wall budget, tightest margin 1.6× (H1-N4-i64); solver pin Kani 0.68.0 / CBMC
6.11.0 / rustc 1.98.0; proof times tracked as metrics, not just pass/fail. Pinned
by the harnesses plus the mutation-red log.

**R33 (verdict recorded).** The probe verdict is adopted as stated in R32 — no
`pending` remains. Part A lands the i64 migration, the twelve harnesses, and the
H6 measurement with its count; if H6 misses the rule, the fallback is proptest
plus segment-local reasoning, recorded as a design erratum, not a silent drop.

## 9. Staging

Three parts, three commits (MO1 is extra large — MO0 §8/§10: split rather than
thin). Every commit runs the green workspace gate per AGENTS.md (`build
--workspace`, `test --workspace`, `fmt -- --check`, `clippy --workspace
--all-targets -- -D warnings`) plus two Opus reviews per stage (MO0 §0).

- **Part A — core model + literals** (`kinewright-core`): R1/R4/R5/R6/R7 model
  changes, `EffectWire` fields, R11–R14 survival + relaxation, R15–R19 operations,
  R30 incident-table row, R31 i64 migration + sweep, R32 twelve harnesses + H6
  measurement. Plus the critic-measured literal passes, counted explicitly in this
  part's budget: ~200 `MediaKind::` sites, 366 `Clip {` + 430 `Effect {` literals
  across crates (mechanical field defaults, counted as edit sites). Registry moves
  in this commit (six generated mutators embed `Operation`); R21 sextuple rows and
  pin updates land here, not later. Gate: core contract tests, sweep + differential
  green, served-quad pins green.
- **Part B — media render + skips** (`kinewright-media`): R4/R5 skip arms in
  `timeline.rs` and every R4 reader, R2/R3 fold + WGSL, R7/R8 probe + still hold,
  R26–R29 lavapipe goldens and export round trip. Gate: all §6 goldens green on
  lavapipe, transform-less identity byte-exact.
- **Part C — person + agent** (`kinewright-app`, `kinewright-agent`): R22–R25 GUI,
  R20 `plan_motion` with budget asserts, R21 M36 ledger prose, DESIGN.md notes.
  Gate: widget/rect tests, planner goldens + budget, parity checklist, hands-on
  session (§12) with findings discharged or deferred.

Line budget (implementation, tests included): ≤ ~4,500 new lines total, ≤ ~1,800
per part; a part overrunning by > 20% splits instead of thinning. This doc holds
≤ ~650 lines through revision.

## 10. Exit gate (named behaviours)

Each fails on main today, for the stated reason; "no regressions" alone gates
nothing (MO0 §0).

1. `push_in_survives_trim_in_then_out` — an eased scale push-in trimmed +20 at the
   head and back renders and resolves byte-identically on keep-outside owners
   (fails: no keep-outside; trim reshapes eased segments). Scoped to keep-outside
   curves only — audio keeps AU4 reshape. Contract test + lavapipe frame pins.
2. `split_copies_keys_to_both_halves` — an eased move split mid-flight plays across
   the cut with both halves holding the full shifted key count (fails: split
   truncates curves). Contract test.
3. `ken_burns_still_renders` — PNG import → fitted 5 s freeze clip → scale/position
   keys → first/mid/last frames match pinned hashes (fails: no `Image` kind).
   Lavapipe.
4. `tangents_round_trip_ignored` — nonzero tangents survive serde + trim/split and
   change no evaluation (fails: fields do not exist). Contract test.
5. `disabled_effect_renders_through` — toggling an effect off equals removing it in
   `visual_layers_at` (fails: no `enabled` flag). GPU-free primary, lavapipe as
   second lane.
6. `disabled_clip_renders_through` — disabling a clip removes it from every layer
   and silences it in the segments (fails: no clip enable). GPU-free primary
   (layer + audio-segment equality), lavapipe second.
7. `single_key_upsert_remove_round_trip` — upsert/replace/remove/what-is-read-back
   through the revision-gated path, including key-move as one atomic batch and
   last-key-to-static (fails: operations do not exist). Agent-path test.
8. `copy_attributes_verbatim` — cross-clip copy reproduces values, keys, and
   `enabled` (fails: operation does not exist). Contract test.
9. `plan_motion_push_in_applies` — `plan_motion` proposes a push-in the caller
   commits through the ordinary path; budget asserts hold (fails: planner does not
   exist). Real-endpoint test.
10. `opacity_band_matches_operation_path` — a band drag produces exactly the curve
    the equivalent operations would (fails: no band). Math test + hands-on.
11. `push_in_step_sub_pixel` — a canonical push-in (R1) steps ≤ 1 px frame-to-frame
    (fails: whole-percent params step ~19 px). Contract test on evaluated effective
    scale + lavapipe pixel-diff.
12. `kani_harnesses_green` — the R32 twelve harnesses green at their pinned bounds
    in the CI Kani job, H6 measured with its count (fails: harnesses do not exist).
    CI job + red log.

Pins (pass today, must not regress): `served_quad_unchanged` — twentieth
measurement still 7 / 5,660 B / 3,510 B / 998 B. Pin sites.

## 11. Deferrals with owners

- Key-lane drag-move, multi-key select/move, key copy/paste, on-canvas position path
  → MO2 (overlay-manipulation work); lanes paint/select/delete here (R23).
- Downscale-on-import for >8192 px stills → future media work (probe refuses, R7).
- `time_remap` milli-frames exceed the ±1e6 proven range after ~33 s → MO4
  re-probes wider or bounds the values (S1 flag).
- CPU-reference twin for transform + fps/VRAM budget → MO2 (R26 re-gated there).
- Bezier evaluation and handle UI → AE programme (fields reserved, R6).
- Multi-turn rotation (beyond ±360°) and off-layer anchor pivots → later
  descriptor-only widening on demand (no migration; R1).
- Image-sequence import (numbered runs as one asset) → future media-depth slice;
  single stills only here.
- `plan_motion` presets beyond the R20 six (whip-pans, match-cuts, shake) → MO6
  preset mechanism.
- Tracker-driven transform animation → MO5 (rides `track_region`, decimated).
- Title entrance/exit animation → MO6 (titles already carry `Clip.effects`, so the
  MO1 model covers them; presets and the typewriter field come later).
- Motion blur → AE programme, shutter-accurate only; no approximation (MO0 §11).

## 12. Hands-on checklist for Riel

The slice is not complete until this session's findings are discharged or recorded
as deferrals (MO0 §0). Script (~30 min):

1. Import a portrait still and a landscape still: bin badges, 5 s spans, fitted
   scales — do the baked values look right?
2. Ken Burns by hand (auto-key edits, prev/next-key nav): does the move feel
   intentional, not mechanical? (the MO0 §9 creative question for push-ins).
3. Push-in on a shot, trim in 20 frames, trim back out: is the move untouched?
4. Split a keyframed clip mid-move: does the move play across the cut?
5. Toggle an effect off/on mid-playback; disable a clip (dimmed?): instant and
   obvious?
6. Opacity band drag on a video clip; key-lane diamonds select/delete: does the
   band fight clip selection or drags?
7. Copy attributes between two clips (with and without keys): verbatim?
8. Agent run: `plan_motion` push-in and `ken_burns` end to end (plan → commit →
   proof): were you asked anything but creative questions?
