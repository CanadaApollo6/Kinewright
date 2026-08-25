# CC5 secondaries

Status: implementation contract, 2026-08-25 (reviewed; critic corrections applied)
Depends on: [CC0](ROADMAP-AND-WORKFLOWS.md), [CC1 managed SDR primary](CC1-MANAGED-SDR-PRIMARY.md), [CC2 scopes and matching](CC2-SCOPES-AND-MATCHING.md), [CC3 curves and wheels](CC3-CURVES-AND-WHEELS.md), [CC4 look management](CC4-LOOK-MANAGEMENT.md), [M40 generalization gauntlet](M40-GENERALIZATION-GAUNTLET.md)
Scope: a **matte that belongs to a correction node** — geometric windows, an HSL qualifier, feathering, keyframes, tracking, matte inspection, and matte-scoped scopes — inside the CC1 managed SDR pipeline.

CC5 does not change CC1's input, working, monitoring, or delivery contract. It gives four of the five CC3/CC4 node kinds an optional per-node coverage function `m ∈ [0,1]` and gates that node's transform by it. Every CC1 invariant is preserved verbatim, and **invariant 2.2.4 — "alpha and geometry are independent of RGB colour correction; a layer mask is not a CC1 secondary and must not be used as one" — is the reason this slice exists.** No CC5 code path writes alpha.

The words **must**, **must not**, and **may** in this document are normative.

## 1. In scope and out of scope

CC5 delivers:

- an optional **matte** on `primary_correction`, `color_wheels`, `color_curves`, and `creative_look`, expressed as 47 integer parameters on the same effect;
- up to four geometric **windows** (rect / ellipse) with centre, half-extents, rotation, symmetric feather, per-window invert, combined by union or intersection, in an aspect-corrected normalized space with exact CPU/GPU formulas;
- an **HSL qualifier** (hue / saturation / luma bands with softness) evaluated on the value *entering* the node, in the CC3 `grade709` encoding;
- matte **invert** and **mix**, and the single normative application rule `out = x + (node(x) − x)·m`;
- a widened GPU node-stack ABI (`GRADE_ABI_VERSION` 2 → 3) carrying a 64-word matte block in the existing payload region of the existing single storage buffer;
- an independent CPU reference `apply_color_nodes_at(nodes, rgb, uv, aspect)`;
- **matte inspection**: a new `Analysis::matte_proof_for_document` rendered by the production compositor, an `inspect_grade_matte` agent tool, and matte-scoped scopes fed to the unchanged CC2 engine;
- **tracking and keyframes**: `track_matte_window`, reusing the existing tracker with the M40 smoothing policy, as a prepared plan that is never committed;
- a human matte section, preview window overlay with drag editing, and a matte-view toggle; and
- a fixture suite whose central gate is **affected-pixel containment**: bit-identical pixels outside the matte, on CPU and GPU.

CC5 does **not** deliver automatic subject, face, skin, or object detection, segmentation, or any ML matte; hue-vs-hue / hue-vs-sat / sat-vs-sat curves (deferred with CC3's list); spatial blur or softening of the matte (§11); polygon, bezier, or freehand roto; a new tracker algorithm — CC5 bounds and reuses the existing normalized-SAD template matcher and adds no scale, rotation, planar, or occlusion estimation; gamut, legal-range, or skin QC (CC6); manual keyframe authoring UI or a timeline keyframe editor (§11); or matte sharing between nodes.

**The layer `mask` effect is unchanged.** It stays a compositing alpha operation applied last in the fragment shader, keeps its parameters, its rendering, and its reporting, and is *not* a colour node. CC5 adds the thing the roadmap says is missing, next to it, without touching it. The inspector labels them "Mask (layer alpha)" and "Matte (this correction)" so the distinction is visible rather than folklore.

## 2. Matte architecture

### 2.1 Which nodes may carry a matte

| Effect name | Matte | Reason |
| --- | :---: | --- |
| `technical_lut` | **no** | A technical input transform normalizes the *whole* source. A partially applied source normalization is not a meaningful state — the same argument that pins its `mix_basis_points` (CC4 §5.1). Its descriptor carries no `matte_*` parameter, so a `SetEffectParam` naming one is the ordinary `UnknownEffectParameter` rejection. |
| `primary_correction` | yes | |
| `color_wheels` | yes | |
| `color_curves` | yes | |
| `creative_look` | yes | A vignette-shaped or sky-only look is an ordinary grading request. |

Rejected alternative: a separate `matte` effect that references a node by id. Rejected because the roadmap's architecture principle is that the matte *belongs to* its correction, and because same-effect storage makes undo, keyframes, reset, bypass, the proof manifest, and `RemoveEffect` trivially consistent — a separate effect would need a retargeting operation, a dangling-reference invariant, an ordering rule relative to its node, and a second inactive-node concept, for no expressive gain. Rejected alternative: a `ParamValue::Matte` struct variant. Rejected for CC3 §2.4's reasons, unchanged.

### 2.2 Parameters

All parameters are `ParamValue::Integer` with `uniform: EffectUniform::ColorNode`. Bounds are inclusive; an omitted parameter resolves to its neutral. Names are generated from two patterns, as CC3 generates the 133 curve parameters, so the table is built rather than transcribed.

Matte controls, 15 parameters:

| Parameter | Stored unit | Min | Max | Neutral | Meaning |
| --- | --- | ---: | ---: | ---: | --- |
| `matte_enabled` | boolean token | 0 | 1 | 0 | Master switch. `0` makes the node's coverage exactly `1` and the node byte-identical to CC4. |
| `matte_window_count` | count | 0 | 4 | 0 | Active windows. `0` means no geometric restriction. |
| `matte_combine_token` | enum token | 0 | 1 | 0 | `0` union, `1` intersection. |
| `matte_invert` | boolean token | 0 | 1 | 0 | Complements the combined coverage before `matte_mix`. |
| `matte_mix_basis_points` | 1/10000 | 0 | 10000 | 10000 | Scales the final coverage; also the node's strength inside the matte. `0` makes the node inactive **when `matte_enabled == 1`**; like every other matte control it is ignored when `matte_enabled == 0` (§2.6). |
| `matte_qualifier_enabled` | boolean token | 0 | 1 | 0 | |
| `matte_hue_center_centidegrees` | 1/100 degree | 0 | 35999 | 0 | Hue band centre. |
| `matte_hue_width_centidegrees` | 1/100 degree | 0 | 18000 | 18000 | Half-width. `18000` (180°) **disables the hue leg entirely**, including for achromatic pixels (§2.4). |
| `matte_hue_softness_centidegrees` | 1/100 degree | 0 | 18000 | 0 | Shoulder width beyond the half-width. |
| `matte_saturation_low_basis_points` | 1/10000 | 0 | 10000 | 0 | |
| `matte_saturation_high_basis_points` | 1/10000 | 0 | 10000 | 10000 | |
| `matte_saturation_softness_basis_points` | 1/10000 | 0 | 10000 | 0 | |
| `matte_luma_low_basis_points` | 1/10000 | 0 | 10000 | 0 | |
| `matte_luma_high_basis_points` | 1/10000 | 0 | 10000 | 10000 | |
| `matte_luma_softness_basis_points` | 1/10000 | 0 | 10000 | 0 | |

Window controls, 8 parameters per window, `{j}` ∈ `0..=3`:

| Parameter | Stored unit | Min | Max | Neutral | Meaning |
| --- | --- | ---: | ---: | ---: | --- |
| `matte_window{j}_shape_token` | enum token | 1 | 2 | 1 | `1` rect, `2` ellipse. The `mask.shape_token` vocabulary, deliberately reused. |
| `matte_window{j}_center_x_basis_points` | 1/10000 of frame width | -10000 | 20000 | 5000 | Off-frame centres are legal so a tracked window may leave and re-enter, exactly as CC3 §2.3 lets a curve point sit outside `0..10000`. |
| `matte_window{j}_center_y_basis_points` | 1/10000 of frame height | -10000 | 20000 | 5000 | |
| `matte_window{j}_half_width_basis_points` | 1/10000 of frame width | 1 | 10000 | 2500 | |
| `matte_window{j}_half_height_basis_points` | 1/10000 of frame height | 1 | 10000 | 2500 | |
| `matte_window{j}_rotation_centidegrees` | 1/100 degree | -18000 | 18000 | 0 | Clockwise as the viewer sees it (§2.3). A window is symmetric under 180°, so this covers every orientation. |
| `matte_window{j}_feather_basis_points` | 1/10000 of the normalized field | 0 | 10000 | 0 | Symmetric band straddling the shape edge. |
| `matte_window{j}_invert` | boolean token | 0 | 1 | 0 | Complements this window only, before the combine. |

**47 parameters per matte-carrying node.** Descriptor sizes become `primary_correction` 57, `color_wheels` 60, `color_curves` 180, `creative_look` 51. Points at index `>= matte_window_count` are ignored by rendering, are preserved byte-for-byte if stored, and **should** be omitted from project JSON, exactly as CC3 §2.4 handles inactive curve points.

**Schema compactness (M36), normative.** `schema.rs` **must not** enumerate the 32 window parameters per kind. It emits one shared legend:

```text
matte_window{j}_* for j in 0..=3: shape_token 1..2 (1 rect, 2 ellipse);
center_x/y_basis_points -10000..20000 (neutral 5000);
half_width/half_height_basis_points 1..10000 (neutral 2500);
rotation_centidegrees -18000..18000; feather_basis_points 0..10000;
invert 0..1. See inspect_grade_matte for measured coverage.
```

the same special case `color_curves` already receives.

### 2.3 Windows: exact geometry

Let `u ∈ ℝ²` be the **layer output quad uv**, with `u.y = 0` at the top — the same `input.uv` the legacy mask uses, so the window follows the picture through `scale` and `offset`. The preview overlay draws in composite space through `image_rect`, so it converts between composite and layer uv with the §5.2 formulas using the clip's `transform` evaluated at the same frame; the two coincide only at identity transform. Let `a = W / H` be the **output raster aspect**, supplied by the host (§3.2), not sniffed from `textureDimensions`.

Define the height-normalized pixel offset from the window centre `(cx, cy)` (uv units):

```text
d = ( (u.x - cx) * a , (u.y - cy) )
```

`d` is the pixel offset divided by `H`, so it is isotropic in pixels. Rotate rigidly by the window angle `θ` (`cosT`, `sinT` computed once on the host in f64 and stored as f32, so both implementations consume identical constants):

```text
q.x =  d.x * cosT + d.y * sinT
q.y = -d.x * sinT + d.y * cosT
n   = ( q.x / (hw * a) , q.y / hh )
```

where `hw = half_width_basis_points / 10000` and `hh = half_height_basis_points / 10000`. `hw · a` and `hh` are both half-extents in units of raster height, so `n` is dimensionless with the boundary at `|n| = 1`.

```text
D_rect    = max(|n.x|, |n.y|)
D_ellipse = sqrt(n.x² + n.y²)
```

**Because the half-extents are basis points of width and of height respectively, the aspect factor cancels exactly at `θ = 0`: `n = ((u.x − cx)/hw, (u.y − cy)/hh)`, the plain per-axis normalized rect. The field is isotropic *in pixels* only when `hw·a == hh`.** The aspect correction exists solely to keep rotation rigid — without it a 45° rotation on a 16:9 raster shears the window into a parallelogram. §9's rotation fixture is therefore the aspect gate. An ellipse is a circle in pixels exactly when `hw · a == hh`; the inspector shows a "make circular" affordance that writes that relation.

**Feather.** With `f = feather_basis_points / 10000` and the exact `smoothstep(A, B, x) = t·t·(3 − 2t)`, `t = clamp((x − A) / (B − A), 0, 1)`:

```text
w = (D <= 1) ? 1 : 0                      if f <= 0
w = 1 - smoothstep(1 - f, 1 + f, D)       if f > 0
```

The band straddles the edge, so `w = 1` for `D ≤ 1 − f`, `w = 0` for `D ≥ 1 + f`, and `w = 0.5` exactly at `D = 1`. Chosen because it makes `1 − w` the exact complement of the same shape with the same band, so `invert` is a true complement and the containment gate does not depend on which side the band fell. The `f == 0` hard branch is mandatory: `smoothstep` with `A == B` is undefined. Rejected alternative: separate inner and outer softness (four more parameters per window). Rejected because it breaks that symmetry and because a colourist adjusting one number is the common case.

Rejected alternative: feather in frame units rather than field units. Rejected because a rect's `x` and `y` fields have different pixel scales, so a single frame-unit feather would need two band widths and a per-edge normalization the shader would have to recompute per pixel.

Per-window weight and combine:

```text
w'_j       = window{j}_invert ? 1 - w_j : w_j
m_windows  = 1                              if window_count == 0
           = max_j w'_j                     if combine_token == 0 (union)
           = min_j w'_j                     if combine_token == 1 (intersection)
```

`max`/`min` are exact at 0 and 1 and idempotent, so the affected set is exactly the union or intersection of the shapes — which is what the containment gate asserts. Rejected alternative: screen compositing `1 − ∏(1 − w_j)`. Rejected because it is not idempotent and leaks coverage wherever two feather bands overlap.

Degenerate resolved values are **not reachable through keyframes** (every keyframe value is validated against the descriptor, and `half_*_basis_points` has minimum `1`, so linear interpolation stays in bounds), but rendering must not fail on a hostile or future buffer: `hw <= 0` or `hh <= 0` makes that window's `w = 0`, defensively. No error, no clamp, mirroring CC3 §3.4.

### 2.4 Qualifier: exact colour maths

The qualifier evaluates on the value **entering the node**, before that node's own transform. Justification: each managed node is self-contained (CC1 §3, CC3 §3.1) and consumes only its input, so a qualifier on the node input keeps the node a pure function of its input and keeps the GPU loop stateless. Judging on the *original frame* instead would require carrying a second RGB triple through the whole stack, would make a node's matte depend on nodes placed before it in ways the manifest could not express, and would make "select the skin *after* the primary balanced it" impossible — which is exactly the ordinary workflow.

It evaluates in the CC3 `grade709` encoding, on a **local clamped copy**, exactly as `chroma_key` clamps a local display-coded copy while the pipeline value stays unclamped (CC1 §2.2.5 is preserved; the clamp never escapes this function):

```text
e = ( grade709_encode(x.r), grade709_encode(x.g), grade709_encode(x.b) )
c = clamp(e, 0, 1)
M = max(c.r, c.g, c.b)
mn= min(c.r, c.g, c.b)
C = M - mn
S = 0                if M <= 0
  = C / M            otherwise
Y = 0.2126*c.r + 0.7152*c.g + 0.0722*c.b
```

`grade709` is chosen because CC3 already defines it as an exact bijection with no clamp, and because hue and saturation thresholds are only meaningful display-referred: in scene-linear light almost every skin tone sits in the bottom eighth of the luma range. `S` is HSV-style `C / M` with an explicit zero rule, not HSL's `C / (1 − |2L − 1|)`, which diverges near white. `Y` is the Rec.709 luma already used by the legacy shader, the scopes, and the monitor path; a second lightness definition would be unreadable in a proof manifest. **This is a deterministic selector, not a perceptual model. It is not Delta E and makes no skin-tone claim.**

Hue, in degrees, with the standard hexagonal formula:

```text
H = undefined                                if C == 0
  = 60 * (((c.g - c.b) / C) mod 6)           if M == c.r
  = 60 * ((c.b - c.r) / C + 2)               if M == c.g
  = 60 * ((c.r - c.g) / C + 4)               if M == c.b
```

Ties (`M` attained by more than one channel) take the first matching branch in that written order, so both implementations agree.

**Achromatic rule, normative.** With `Hc = hue_center/100`, `Hw = hue_width/100`, `Hs = hue_softness/100`:

```text
if Hw >= 180:                       h = 1          (hue leg disabled; achromatic included)
else if C == 0:                     h = 0          (hue undefined -> excluded)
else:
    dh = |H - Hc| ; dh = min(dh, 360 - dh)         in [0, 180]
    h  = (dh <= Hw) ? 1 : 0                        if Hs <= 0
    h  = 1 - smoothstep(Hw, Hw + Hs, dh)           if Hs > 0
```

The `Hw >= 180` escape is why the neutral of `matte_hue_width_centidegrees` is `18000`: a qualifier that constrains only saturation and luma must not silently drop every grey pixel, and a qualifier that names a hue must not silently select every grey pixel. Both behaviours are needed and neither can be the default, so the boundary of the range is the switch and it is a numeric test, not a special-cased flag.

Saturation and luma bands, with `lo`, `hi`, `s` in `0..1`:

```text
band(v, lo, hi, s):
    if lo > hi:  return 0                          (degenerate; §2.6)
    if s <= 0:   return (lo <= v && v <= hi) ? 1 : 0
    return min( smoothstep(lo - s, lo, v), 1 - smoothstep(hi, hi + s, v) )
```

`min` rather than a product so the band interior is exactly `1` and the two shoulders cannot interact.

```text
qualifier = h * band(S, sat_lo, sat_hi, sat_s) * band(Y, luma_lo, luma_hi, luma_s)
```

Multiplication is the conventional qualifier composition; the affected set is identical under `min`, since a product is zero exactly when a factor is zero.

### 2.5 The one place a matte touches RGB

```text
m_raw = m_windows * qualifier                (qualifier = 1 when disabled)
m_inv = matte_invert ? 1 - m_raw : m_raw
m     = m_inv * matte_mix
out   = (m == 0.0) ? x : x + (node(x) - x) * m
```

Normative:

1. **This is the only place a matte affects anything.** `m` is never written to alpha, never multiplied into alpha, never used as a compositing weight, and never leaves the node.
2. **Alpha is not modified by any CC5 code path.** CC1 §2.2.4 holds unchanged. §9's always-on alpha assertion is the gate, not the prose.
3. **No RGB clamp is introduced.** `node(x)` is CC1/CC3/CC4 maths unchanged, and the blend is a linear-light lerp, so CC1 §2.2.5 holds. Over-range and negative values survive a matte exactly as they survive a node.
5. **`m == 0` is an exact identity, per pixel.** When the resolved coverage at
   a pixel is exactly `0`, the node's transform **must not** be blended in: the
   shader and the CPU reference return `x` unmodified. This is not an
   optimization. `x + (node(x) − x)·0.0` is *not* the identity in f32: it maps
   `−0.0` to `+0.0` (different `to_bits`), and it maps any `x` to `NaN`
   whenever `node(x)` is `±inf` or `NaN`, which CC1 §2.2.5's no-clamp rule
   makes reachable on over-range input (a wheels power of an over-range value,
   a LUT domain division). §9.2.1's `to_bits`-identical outside-pixel gate
   depends on this clause, and §9.2.1 **must** include an over-range sample
   whose node output is non-finite and assert it is bit-identical outside the
   matte.
4. **A node without an active matte must not take this path at all.** The shader and the CPU reference **must** skip the blend when the node carries no matte block, because `x + (y − x)·1.0` is not bit-identical to `y` in f32. This is the CC5 analogue of CC3 §3.3, and it is what makes "a pre-CC5 project renders bit-identically" provable rather than approximate.

`matte_mix_basis_points` scales the final coverage rather than blending the matte toward `1`. Rejected alternative: `m = 1 − mix + mix·m_raw`. Rejected because `mix = 0` would then mean "matte off, node everywhere", which is the opposite of every other `mix` in this pipeline and would give the node no strength control at all.

### 2.6 Inactive mattes and inactive nodes

Tested on the stored integers after keyframe evaluation, never on floats.

**The matte is inactive** — no matte block is written, `v11 = 0`, and the node is byte-identical to its CC4 self — when `matte_enabled == 0`, or when `matte_window_count == 0 && matte_qualifier_enabled == 0 && matte_invert == 0 && matte_mix_basis_points == 10000`.

**The node is inactive** with a new `ColorNodeInactiveReason::MatteExcluded` when `matte_enabled >= 1` and either `matte_mix_basis_points == 0`, or `matte_window_count == 0 && matte_qualifier_enabled == 0 && matte_invert == 1` (an inverted empty matte is `m = 0` everywhere). An inactive node is the exact identity: it is not written to the buffer and is skipped by the CPU reference, so bypass and a zero-mix matte are losslessly identical to removing the node, bit for bit.

Degenerate resolved bands (`sat_lo > sat_hi`, `luma_lo > luma_hi`) evaluate to `0` and the inspector reports `matte_band_inverted_by_automation`. No clamping, no reordering, no error — CC3 §3.4's posture.

## 3. Evaluation and GPU mechanism

### 3.1 Node record and matte block

The CC3 §3.2 / CC4 §4.2 record layout is unchanged: 64-byte stride, `[kind, payload_word_offset, bypass, reserved, v0..v11]`, one storage buffer, `COMPOSITOR_REQUIRED_STORAGE_BUFFERS_PER_SHADER_STAGE = 1`. A second storage binding remains forbidden (downlevel), so the matte lives in the existing payload region.

**`v11` becomes `matte_payload_word_offset`.** It is free on every kind today: primary uses `v0..v9`, wheels `v0..v8`, curves none, LUT `v0..v10` with `v11` reserved `0.0`. `0` means *no matte*; a non-zero value is an index into `words`, unambiguous because `words[0]` is always the first record's `kind`. Chosen over "the matte block follows the curve payload" because a self-describing offset does not couple the shader to the curve payload's length and needs no per-kind arithmetic.

`payload_word_offset` (record word 1) keeps its CC3 meaning: the curve payload, `0` for every other kind. Payloads are appended in node order; for one node, curve payload first, then matte block.

**Matte block, 64 words (256 bytes), exact:**

| Word | Meaning |
| ---: | --- |
| 0 | `window_count`, `0..=4` |
| 1 | `combine_token`, `0` union / `1` intersection |
| 2 | `qualifier_enabled`, `0` or `1` |
| 3 | `matte_invert`, `0` or `1` |
| 4 | `matte_mix`, `0.0..=1.0` |
| 5 | `raster_aspect` `a = W/H`, host-supplied, `> 0` |
| 6 | `hue_center`, degrees |
| 7 | `hue_width`, degrees |
| 8 | `hue_softness`, degrees |
| 9 | `sat_low`, `0..=1` |
| 10 | `sat_high` |
| 11 | `sat_softness` |
| 12 | `luma_low` |
| 13 | `luma_high` |
| 14 | `luma_softness` |
| 15 | reserved, `0.0` |
| `16 + 12j + 0` | window `j` `shape`, `1` or `2` |
| `+1`, `+2` | `cx`, `cy` in uv |
| `+3`, `+4` | `hw`, `hh` in uv |
| `+5`, `+6` | `cosT`, `sinT`, host-solved in f64, rounded once to f32 |
| `+7` | `feather`, `0..=1` |
| `+8` | `invert`, `0` or `1` |
| `+9 .. +11` | reserved, `0.0` |

`16 + 4 × 12 = 64` words. Inactive windows (index `>= window_count`) are written as zeros and never read.

**Arithmetic.** Worst case is sixteen curve nodes each carrying a matte:

```text
16 header
+ 16 * 64                      = 1024   node records
+ 16 * (4 * 49 * 4)            = 12544  curve payloads
+ 16 * (64 * 4)                = 4096   matte blocks
                               = 17680 bytes
```

`GRADE_BUFFER_WORST_CASE_BYTES` becomes `17_680` (from `13_584`), which exceeds the current binding size, so **`COMPOSITOR_REQUIRED_STORAGE_BUFFER_BINDING_SIZE` becomes `32_768`** — the next power of two, following CC3's stated convention, and far below every supported adapter's advertised limit. The binding *count* stays `1`. `COMPOSITOR_REQUIRED_TEXTURE_DIMENSION_3D` and the LUT atlas are untouched.

`GRADE_ABI_VERSION` goes `2 → 3`, because a consumer that understands only the CC4 kinds would read `v11` as a reserved zero and silently render an unmasked correction.

### 3.2 Host plumbing

`grade_buffer_bytes_with_luts(effects, library)` becomes `grade_buffer_bytes_with_luts(effects, library, raster_aspect, matte_debug_node)`. `Compositor::composite(width, height, layers, library)` already owns the output resolution, so `raster_aspect = width as f32 / height as f32` is computed once per render and is the same value the CPU reference receives. The layer quad is scaled uniformly in NDC, so its pixel aspect equals the output raster aspect regardless of `params.scale`; that identity is asserted in a fixture.

`header.w`, reserved `0` since CC3, becomes the **matte-debug selector**: `0` is normal rendering, `k > 0` means "return the coverage of active node `k − 1` instead of colour". This is why the matte visualization needs no `LayerParams` word (there are none free after CC4), no second pipeline, and no new binding. The active index is resolved **at the requested frame**, after keyframe evaluation, because an inactive node is not written to the buffer and therefore shifts the indices of the nodes after it; a proof request whose target node is inactive at that frame fails typed with `matte_proof_node_inactive { reason }` rather than selecting a different node.

### 3.3 Shader

`compositor.wgsl` gains `matte_window_weight`, `matte_qualifier_weight`, and `matte_coverage(base, uv, rgb_in) -> f32` transcribing §2.3/§2.4 verbatim, and one `var<private> matte_debug_coverage: f32;`.

`apply_color_nodes(input_rgb)` becomes `apply_color_nodes(input_rgb, uv)`; `aspect` is read from the matte block, not passed, so a matte-free stack costs nothing. Per node:

```text
let matte_base = u32(round(words[base + 15u]));
if matte_base == 0u {
    corrected = <kind dispatch>(corrected);          // CC4 path, untouched
} else {
    let m = matte_coverage(matte_base, uv, corrected);
    if index + 1u == grade_buffer.header.w { matte_debug_coverage = m; }
    corrected = corrected + (<kind dispatch>(corrected) - corrected) * m;
}
```

The `matte_base == 0` branch is normative, not an optimization (§2.5.4). An unrecognized kind stays the identity, and the existing fixture asserting every `ColorNodeKind` has a shader branch is extended to assert every matte-capable kind takes the matte branch.

`fragment_main` returns `vec4(c, c, c, 1.0)` immediately after the node stack when `grade_buffer.header.w > 0u`, before the legacy stage, the key, the fade, the crop, and the mask, so nothing downstream perturbs the coverage and alpha is forced to `1` so a CC2 measurement cannot discard it.

The layer's geometric stages are unaffected: the matte is evaluated at `input.uv`, the layer's own output frame. A window on a *reframed* clip is therefore anchored to the reframed output, and `track_matte_window` samples composited thumbnails that already include the reframe, so the two are self-consistent. This is stated, not hidden.

### 3.4 CPU reference independence

`color_pipeline.rs` gains `MatteWindow`, `Matte`, and `Matte::coverage(uv, aspect, rgb_in)`, written **independently** of the compositor's block builder and of the shader (CC3's rule). `ColorNode` variants gain `matte: Option<Matte>`.

`apply_color_nodes(nodes, rgb)` is **replaced** by `apply_color_nodes_at(nodes, rgb, uv, aspect)`. The two-argument form is removed rather than wrapped: a wrapper would have to invent a position, and applying a matte-scoped correction to every pixel is precisely the failure CC5 exists to prevent. The reference has no production caller — only `cc3_fixtures.rs`, `cc4_fixtures.rs`, and the module's own tests — so the change is contained. `apply_primary_corrections` keeps its CC1 signature and gains a doc note that it is the matte-free primary path.

Fixtures iterate `(x, y)` and evaluate at the **pixel centre**, `uv = ((x + 0.5) / W, (y + 0.5) / H)`, matching the rasterizer's `@builtin(position)` convention. That correspondence is asserted directly in §9.1.

## 4. Matte inspection

### 4.1 `MatteProof`

Core gains, next to `MonitorProof`:

```rust
pub struct MatteProof {
    pub coverage: RgbaImage,          // R = G = B = round(255 * m), A = 255
    pub metadata: MatteProofMetadata,
}

pub struct MatteProofMetadata {
    pub render: MonitorProofMetadata, // renderer provenance, reused unchanged
    pub clip: ClipId,
    pub effect: EffectId,
    pub node_kind: String,
    pub coverage_encoding: String,       // always "linear_coverage_u8"
    pub coverage_scale: u16,             // 255
    pub raster_aspect_millionths: i64,
    pub matte_enabled: bool,
    pub window_count: u8,
    pub qualifier_enabled: bool,
}
```

and `Analysis::matte_proof_for_document(document, at, clip, effect) -> Result<MatteProof, MediaError>`, defaulting to `MediaError::NotImplemented`.

**`MonitorProofRenderKind` is not extended.** It names the renderer implementation (`GpuPreview` / `TestDouble`), not an output target; adding a `Matte` value would make provenance mean two things at once. `MatteProofMetadata` composes it and states the output target in its own fields.

Rendering, in `engine.rs`, mirrors `monitor_proof_for_document`: an isolated `FrameRenderer`, a scratch document reduced to the target clip's track and clip so no other layer composites over the coverage, `header.w` set to the target node's active index, and a **readback that applies no transfer at all** — `round(255 · clamp(m, 0, 1))` on the `Rgba16Float` surface. That is an integer quantization of a coverage scalar, not a monitoring transform, which is exactly why it needs its own trait method rather than a `MonitorProof` variant. The proof fails typed when the node is inactive (`matte_proof_node_inactive { reason }`) or carries no matte (`matte_proof_no_matte`); it never returns a blank frame.

### 4.2 `inspect_grade_matte`

Read-only. Arguments `expected_revision`, `clip_id`, `effect_id`, `timecode`, optional `include_image` (default `true`). Returns the coverage statistics and, when requested, a PNG `ContentBlock` through the existing `encode_png` / `BASE64` path:

- `covered_pixel_count` (`m > 0`), `full_pixel_count` (code 255), `partial_pixel_count`;
- `covered_basis_points = floor(covered · 10000 / total)`, integer floor as CC2 does for clipping;
- `coverage_histogram`: 16 buckets, `bucket = min(15, floor(code · 16 / 256))`, CC2's bucketing rule;
- `bounding_box_basis_points`: the tightest half-open pixel rect containing every `m > 0` pixel, converted with CC2's ROI floor/ceil rule, or `null` when coverage is empty;
- `centroid_basis_points`, **coverage-weighted** (`Σ m·p / Σ m`) and labelled `weighted_by_coverage: true`. This is a statistic *of the matte*, not a colour measurement, so weighting is correct here and does not contradict CC2's "partial alpha is not a weight", which governs scope inputs;
- `raster`, `raster_aspect_millionths`, the resolved matte parameters, `active`, `inactive_reason`, and the full renderer provenance.

### 4.3 Matte-scoped scopes

**`ScopeStage` is not extended and its fail-closed guard is untouched.** A matte-scoped scope is still measured at `monitoring_post_composite`; only the *region* changes, and CC2 already models a region. Rejected alternative: `ScopeStage::MonitoringPostCompositeMatte`. Rejected because it would claim a pipeline boundary that does not exist and would force `compare_scope_evidence`'s stage equality to mean two different things.

`get_video_scopes_v2` gains an optional request field `matte_region: { clip_id, effect_id }`, composable with `roi` (the measured set is their intersection). The agent path renders the managed monitor frame and the matte proof for the same document, frame, and raster, asserts identical dimensions (else `matte_region_raster_mismatch { observed, allowed }`), and constructs an **analysis-only** RGBA copy with

```text
A = 255 if m > 0 else 0
```

which it hands to the unchanged CC2 engine. The document, the render, and the layer alpha are never touched.

**Threshold rule, pinned: `m > 0`.** A pixel the correction touched at all was affected, so the scope's population is exactly the affected set — the same set §9's containment gate measures. `m >= 0.5` would silently discard half of every feather band and make the scope disagree with the containment fixture. A documented consequence: a very soft qualifier can select nearly the whole raster at a whisper of coverage; `inspect_grade_matte`'s coverage histogram is where that is visible, and the scope response reports `matte_threshold: "coverage_greater_than_zero"` plus `covered_pixel_count` at every level.

`ScopeMeasurementMetadata` gains `matte_region: Option<MatteRegionDescription { clip, effect, threshold, covered_pixel_count }>`, and `compare_scope_evidence` requires both sides to carry the same *requested* region — `clip`, `effect`, and `threshold` — or both none, exactly as it already requires the same stage and ROI. The measured `covered_pixel_count` is deliberately **not** part of that equality: a qualifier matte's coverage is a function of the colour entering the node, so a before/after pair legitimately differs in count; the comparison reports the signed `matte_covered_pixel_delta` (candidate − reference) instead of refusing.

## 5. Tracking and keyframes

### 5.1 Keyframe policy

| Parameters | Policy |
| --- | --- |
| `matte_window{j}_center_x/y_basis_points`, `_half_width_/_half_height_basis_points`, `_rotation_centidegrees`, `_feather_basis_points` | Fully keyframable with any interpolation. These are the tracked and animated controls. |
| `matte_mix_basis_points`, every qualifier scalar (`hue_center/width/softness`, `sat_*`, `luma_*`) | Fully keyframable with any interpolation. |
| `matte_enabled`, `matte_qualifier_enabled`, `matte_window_count`, `matte_combine_token`, `matte_invert`, `matte_window{j}_shape_token`, `matte_window{j}_invert` | **`Hold` only**, enforced in `operation.rs`'s keyframe validator by generalizing the CC3 §6 check: the private `is_curve_point_count_parameter` is renamed `is_hold_only_parameter(effect_name, name)` and gains the `matte_*` token and count predicate alongside its `{curve}_point_count` case (CC3's existing `NonHoldKeyframeParameter` fixture must pass unchanged). Reported as `OpError::NonHoldKeyframeParameter`. Interpolating a token or a count is meaningless — the CC4 `lut_asset_id` precedent. |

`matte_hue_center_centidegrees` interpolates linearly in stored units, so a keyframe pair `35000 → 1000` sweeps *backwards* through 18000 rather than across the seam. That is a documented consequence with a stated recovery ("insert an intermediate keyframe, or use `Hold`"). Rejected alternative: shortest-arc interpolation for this one parameter. Rejected because the keyframe engine is integer-linear and the special case would be invisible in the journal, the manifest, and the curve the user sees.

### 5.2 `track_matte_window`

Arguments: `expected_revision`, `clip_id`, `effect_id`, `window_index` (`0..=3`), optional `start_local_frame`, `end_local_frame`, `step_frames` (default 5, `1..=120`), `search_radius_percent` (default 10, `1..=25`), `max_width` (default 256, `64..=512`), `minimum_confidence_basis_points` (default 5000). `MAX_TRACKING_SAMPLES = 120` is unchanged.

It reuses `track_clip_region` with one generalization: `excluded_effect_name: &str` becomes `excluded_effect: EffectId`. This narrows the exclusion from *every effect with that name* to *exactly one effect*; §9.2.12 asserts the delta (a clip with two `mask` effects tracks identically for the targeted mask while the second mask's alpha is now present in the tracking thumbnails, which is the correct behaviour). **Excluding the node being tracked, by id, is required**: the correction is matte-scoped, so as the window moves the graded picture changes *inside* the window and a SAD template would chase its own output. Excluding exactly that node removes the feedback and leaves every other grade, and every other effect, intact.

The window's tracking box is its axis-aligned bounding box at the first sample, **mapped into the composited thumbnail's frame through the layer transform** (the tracker measures the composite, §5.2 coordinate space): with the layer's resolved `scale` `s`, `box_percent = [2·hw·s·100, 2·hh·s·100]` centred at `u_composite(centre)`, rejected outside `1..=75` with the existing message. Rotation is not tracked and not written. The §9.2.11 fixture asserts this choice at `scale = 0.5` (measured raw error 29/121 bp with the rescaled box).

**Smoothing (M40's lesson, constants pinned here).** Raw tracker centres stutter and let tracker noise become visible motion. `multicam.rs`'s private `stabilized_focus_values` is promoted to `pub fn stabilize_tracked_centres_basis_points(observations: &[i64], minimum: i64, maximum: i64, dead_zone: i64, maximum_step: i64) -> Vec<i64>` — a pure rename with the body unchanged (the three-sample median filter plus reactive controller); `plan_subject_reframe_scaled` and `containment_aware_focus_values` adopt the new name and the existing multicam fixtures are asserted byte-identical across it. CC5 calls it with:

| Constant | Value | Reason |
| --- | ---: | --- |
| `MATTE_TRACK_DEAD_ZONE_BASIS_POINTS` | `0` | A dead zone deliberately lags. That is right for a virtual camera and wrong for a matte, which must stay on the subject. |
| `MATTE_TRACK_MAX_STEP_BASIS_POINTS` | `800` | 8 % of the frame between samples; at the default 5-frame step, 1.6 % per frame — well above ordinary subject motion, while still rejecting a tracker jump to a false match across the frame. |
| median filter | on | Rejects one-sample noise, M40's first fix. |
| bounds | `-10000 ..= 20000` | The parameter bounds, so a subject may leave frame. |
| interpolation | `Linear` | Sustained movement gets continuous velocity; M40 rejected eased per-segment curves. |

**Known systematic lag, stated.** The median filter replaces the final sample with `median(o[n−3], o[n−2], o[n−1])`, so the last smoothed value lags a moving subject by one inter-sample displacement; `MATTE_TRACK_MAX_STEP_BASIS_POINTS = 800` bounds the per-sample motion the controller can follow. Both are accounted for in §9.2.11's tolerances rather than hidden.

**Coordinate space, normative.** `track_matte_window` measures on the composited thumbnail, whose uv is the *output* frame; the matte is evaluated at the *layer* uv. The tool **must** convert `u_layer = (u_composite − 0.5)/scale − (offset_x, offset_y)/(2·scale) + 0.5` (forward: `u_composite = scale·(u_layer − 0.5) + (offset_x, offset_y)/2 + 0.5`; the vertex shader's `−offset_y` in NDC is already absorbed by the `uv.y = (1 − ndc.y)/2` flip, so **no** extra sign on `offset_y` — an erratum corrected during review, the earlier text had `−offset_y`) using the layer's resolved `scale`/`offset` at the sampled frame, and **must** fail typed with `matte_track_layer_transform_unsupported { field, observed, allowed }` when the layer carries a keyframed `scale` or `offset` over the tracked range. The tracker's `extent − 1`-denominated basis points convert to the matte's fraction-of-extent basis points as `centre_bp = round((pixel + 0.5) · 10000 / extent)`. §9.2.11 asserts both at `scale ∈ {0.5, 1.0}`.

The offline lookahead containment solver is **not** used: it exists to keep a subject inside a delivery crop, a constraint a matte window does not have. That is stated so the omission is a decision, not an oversight.

Samples below `minimum_confidence_basis_points` are dropped and reported in `low_confidence_samples`. Fewer than two surviving samples returns `tracking_confidence_too_low { observed, allowed, recovery_action }` — the roadmap's "manual fallback when confidence is low".

The response carries `observations` (local frame, project frame, centre in basis points, `confidence_basis_points`), the smoothed `curves`, the smoothing constants under `window_stabilization`, and a **`tracking_boundary`** statement mirroring M40's provenance marker:

> tracks the explicitly supplied window rectangle by normalized SAD template match on composited thumbnails; no learned object, face, or skin detection, no scale or rotation estimation, and no occlusion handling. `rotation_centidegrees`, `half_width_basis_points`, and `half_height_basis_points` are never written.

It emits two `SetEffectKeyframes` operations through `prepare_operations` as a **prepared edit plan** and **commits nothing**, exactly like `track_mask_region`.

## 6. Human UI

**Matte section** on each of the four matte-capable node cards, collapsed by default so a CC4 project's inspector is unchanged: an enable toggle; a window list with Add / Remove up to four and a per-window row (shape, centre X/Y, half width/height, rotation, feather, invert); a combine selector; qualifier controls with an enable toggle; matte invert; and the mix slider. The generic slider loop hides every `matte_*` parameter (`should_render_effect_parameter` gains the four kinds), because 47 raw sliders is not a workflow.

Every matte control carries the CC3 keyframe indicator and a **Clear keyframes** action, and while a control is keyframed, editing it writes the *static* value with the existing warning. Manual keyframe authoring is **deferred** (§11): CC5 adds no "set keyframe at playhead" control, and the UI says so rather than implying it.

**Preview overlay.** `preview_ui.rs`'s viewer gains `Sense::click_and_drag()` and an overlay layer, both active **only** while a matte section is expanded for the selected clip; otherwise the viewer keeps today's behaviour exactly. Window outlines are drawn through the existing `image_rect` letterbox transform, with the feather band shown as a second dashed outline at `D = 1 ± f`. A **Matte view** toggle replaces the picture with the coverage image, fetched through a `pub(crate) MatteProofSource` trait (in the new `matte_overlay_ui.rs`, modelled on the private `ScopeProofSource` in `color_scopes_ui.rs`, with an `AnalysisMatteProofSource` impl) so the panel stays testable without a window; `preview_ui.rs` and `color_scopes_ui.rs` both consume it.

Hit-testing, in screen pixels, tested in this priority order: within 8 px of the rotation handle, 24 px outside the top edge midpoint → rotate; within 8 px of one of eight edge/corner handles → resize that axis or corner; within 8 px of the centre handle, or anywhere inside the window → move. (Rotate and resize must be tested before move because the edge handles sit on the boundary of "inside the window"; move-first would make every handle unreachable.) Handles are drawn and hit-tested only for the selected window; an unselected window only offers move, which selects it. Drags invert the letterbox transform, `uv = (pointer − image_rect.min) / image_rect.size()`, then the layer transform (§2.3), round half away from zero to basis points, and clamp to the descriptor bounds. The overlay is drawn only while the playhead lies inside the selected clip.

Coalesced undo keys, one gesture = one undo entry through the existing coalesced batch path (a move writes two parameters, so it uses the multi-operation live push):

```text
matte_window_move:{clip}:{effect}:{j}
matte_window_resize:{clip}:{effect}:{j}
matte_window_rotate:{clip}:{effect}:{j}
matte_mix:{clip}:{effect}
```

**"Track window…"** is present but **disabled**, with a tooltip stating that tracking is agent-driven in CC5 and that the app displays the resulting keyframes once the plan is committed. The app has no agent-tool call path, and inventing one for this button is out of scope; pretending the button works would be worse than saying so.

## 7. Agent surface

`INSPECTOR_TOOL_NAMES` grows 71 → 74 (the CC4 text said 70; the pre-CC5 count is 71). `CAPABILITY_KIND_OVERRIDES` gains `("inspect_grade_matte", CapabilityKind::Inspector)` — the `inspect_` prefix matches no inference rule, and name-prefix inference is not a contract. Errors follow the CC1/CC2 shape: `field`, `observed`, `allowed`, `recovery_action`.

- **`inspect_grade_matte`** — §4.2. Read-only, mutates nothing.
- **`track_matte_window`** — §5.2. Returns a prepared plan; commits nothing.
- **`plan_secondary_correction`** — evidence-only, revision-gated, modelled exactly on `plan_color_wheels`. Arguments: `expected_revision`, `clip_id`, either `target_effect_id` or `node_kind` (one of the four matte-capable kinds), optional `append`, and ergonomic `windows: [{shape, center_x, center_y, half_width, half_height, rotation, feather, invert}]`, `qualifier: {...}`, `combine`, `invert`, `mix_basis_points`. It expands them to `matte_*` `SetEffectParam`s (or one `AddEffect` carrying only the requested non-neutral values), validates count, bounds, and the Hold-only rules **before** constructing any operation, and returns `expected_revision`, `clip_id`, `target_effect_id`, `requested_parameters`, `resolved_parameters`, `operations`, and — because a plan for a 47-parameter matte is not inspectable as integers — `predicted_coverage`, the §4.2 statistics measured on a scratch document. It applies nothing. Following the CC2 rule, an existing node of the requested kind is targeted rather than stacked unless `append: true`.

  An optional `sample_roi` returns the measured hue/saturation/luma statistics of that ROI as **evidence**; only with `derive_qualifier_from_sample: true` does it also propose a qualifier, by this pinned formula and no other: `hue_center` = median hue of visible ROI pixels, `hue_width = 1500`, `hue_softness = 1000`, `sat_low = max(0, p10 − 1000)`, `sat_high = min(10000, p90 + 1000)`, `sat_softness = 1000`, luma likewise. This is a colour picker on an explicitly supplied region, not detection: it looks where it was told and its arithmetic is in this document.

- **`render_color_proof`** gains `matte_comparison: "coverage" | "inside_only" | "outside_only"`, valid only alongside `effect_id` on a matte-carrying node. `coverage` returns the §4.1 proof image; `inside_only` renders the document as stored; `outside_only` renders a scratch copy with `matte_invert` toggled. The manifest states which variant it rendered, and §9.10 asserts the partition.
- **`color_nodes` manifest** entries gain a compact integer `matte` object: `enabled`, `active`, `inactive_reason`, `window_count`, `combine`, `invert`, `mix_basis_points`, `qualifier { enabled, hue_center_centidegrees, hue_width_centidegrees, hue_softness_centidegrees, saturation_low/high/softness_basis_points, luma_low/high/softness_basis_points }`, and `windows: [ ... ]` truncated to `window_count`. Absent entirely when the node carries no matte, so a CC4 manifest is byte-unchanged.
- **Schema compactness (M36)**: §2.2's legend, emitted once per matte-capable kind.

## 8. Migration

1. Pre-CC5 projects load unchanged. No effect is renamed, no parameter is inserted, no node is added. With no `matte_*` parameter stored, `matte_enabled` resolves to its neutral `0`, the matte is inactive, no matte block is written, and §2.5.4's mandatory skip makes the render **bit-identical** to CC4. §9.12 asserts it.
2. The `mask` effect is untouched: same descriptor, same `EffectUniform::Mask*` slots, same final-alpha application, same reporting as a compositing operation. `track_mask_region` keeps working unchanged.
3. `ColorPipelineState` stays `managed_sdr_v1`. Rejected alternative: `managed_sdr_v2`. Rejected for CC3 §9 and CC4 §9's reason — `pipeline_state` describes the source → working → monitoring → delivery contract, not the inventory of node features; CC5 changes no colour description and would immediately fail `delivery.rs`'s managed-delivery check for every existing project with no semantic gain.
4. `GRADE_ABI_VERSION` `2 → 3`; `COMPOSITOR_REQUIRED_STORAGE_BUFFER_BINDING_SIZE` `16384 → 32768`; both asserted in the limit-contract test.
5. Save/reopen, journal replay, branch, undo, redo, and recovery preserve every `matte_*` value, window index, and keyframe byte-for-byte apart from documented JSON defaults.

## 9. Exit fixtures and numeric gates

The gate is a fixture suite in the style of `cc3_fixtures.rs` / `cc4_fixtures.rs`, recorded as `crates/kinewright-media/src/cc5_fixtures.rs`, plus `crates/kinewright-core/tests/cc5_core.rs` and agent/app cases. Every fixture records the git revision, backend, adapter, software-fallback and GPU-claim flags, OS, source profile, node stack, resolved matte parameters, coverage counts, and output hashes.

### 9.0 Fixture-quality rules (CC1/CC2/CC3/CC4 reviews — normative, unchanged)

1. Expected values are written out analytically from §2's equations, either as literal constants here or transcribed independently in f64 in the fixture. A fixture **must not** obtain an expected value by calling `Matte::coverage`, `apply_color_nodes_at`, the compositor, or the shader.
2. Every control at minimum, maximum, and a representative interior value has a numeric expected value. `is_finite()` is never a sufficient assertion.
3. Parity rasters must exercise every control and assert their own coverage.
4. Manifest tolerances are asserted equal to the code constants, never restated as literals.
5. GPU fixtures run on a hardware adapter when no software fallback exists, recording honest provenance; the software lane is the default lane and the two stay distinct.
6. Error assertions check `field`, `observed`, and `allowed`.
7. A non-neutral case that changes fewer than 5 % of CPU-reference samples fails as vacuous. For a matte the gate is **two-sided**: at least 5 % of samples *inside* changed **and** exactly 0 samples outside changed.

**The lavapipe lesson applies unchanged.** CC1 §6.2's pixel-exact sampling clause is load-bearing here: a matte multiplies the *difference* a node made, so a sub-texel filter leak of `2^-15` on an "identity" layer becomes a difference the containment gate would report as an outside-pixel change. The parity gate depends on that clause, not on tolerance width, and no epsilon guard is added to the distance field or the smoothstep.

### 9.1 Rasters

Two, each with a job, both 64 × 36 (aspect `a = 16/9`), pixel centres at `((x + 0.5)/64, (y + 0.5)/36)`:

- **`cc5_field_raster()`** for containment: `r = 0.05 + 0.9·x/63`, `g = 0.05 + 0.9·y/35`, `b = 0.05 + 0.45·(x/63 + y/35)`. Every channel is in `[0.05, 0.95]` and strictly varies — in `x`, in `y`, and in both. **No channel is ever 0**, which is required: CC3's raster contains exact zeros that a gain node leaves unchanged, which would make "exactly 0 outside changed" pass for the wrong reason.
- **`cc5_parity_raster()`** for CPU/GPU parity: the CC3 §10.2 192 samples laid out as a 16 × 12 grid of 4 × 3-pixel blocks. It inherits CC3's value-coverage assertions verbatim (negatives, `0..1`, values above 1) *and* varies in both axes, and asserts that the §9.2 window's edges fall **on** block boundaries in both axes — no parity block straddles the window edge, because a straddling block would half-grade a parity sample and turn the containment gate into a feather test. (Earlier wording said "cross"; the implemented and intended reading is "coincide with".)

The fixture asserts that the CPU reference's `uv` at `(x, y)` is the pixel centre and that the GPU's `@builtin(position)` maps to the same pixel, on both rasters.

### 9.2 Required fixtures

1. **Affected-pixel containment (the central gate).** On `cc5_field_raster`, a `color_wheels` node with `gain_master_thousandths = 1500` and a centred rect window `center = (5000, 5000)`, `half_width = half_height = 2500`, `feather = 0`:
   - the window covers **exactly 576 of 2304 pixels** — columns `x ∈ 16..=47` (32) × rows `y ∈ 9..=26` (18), i.e. **2500 basis points**, derived by hand from `|u.x − 0.5| ≤ 0.25` and `|u.y − 0.5| ≤ 0.25`; no pixel centre lies on the boundary;
   - **≥ 5 % of inside pixels changed** (all 576 do) and **exactly 0 of the 1728 outside pixels changed**, asserted `f32::to_bits`-identical in linear working values *and* byte-identical in monitor RGBA8, on the CPU reference and on the GPU;
   - the alpha channel is byte-identical to the same stack with the matte removed;
   - `matte_invert = 1` swaps the two sets exactly: 1728 changed, 576 bit-identical;
   - the same case repeated on a raster variant containing `−0.0` and an
     over-range sample whose node output is `±inf` outside the matte (a wheels
     power on a `4.0` input), asserting those outside pixels are bit-identical
     — the §2.5.5 gate. The `±inf` half is asserted on both CPU and GPU; the
     `−0.0` half is asserted on the CPU reference only, because the GPU
     working-surface upload/sample path normalises `−0.0` to `+0.0` before the
     node stack runs (measured: the no-node baseline already carries `+0.0`),
     so the GPU fixture asserts bit-equality against the no-node render plus a
     genuine negative sample surviving.
2. **Window geometry anchors**, each hand-derived above and asserted on CPU and GPU:
   - centred rect `2500/2500` → **576** pixels;
   - centred pixel-square `half_width = 1125`, `half_height = 2000` (`hw·a = hh = 0.2`; 7.2 px each way), rect, rotation `0` → **196** pixels (`|d| ≤ 7.2` in both axes); rotation `4500` (45°) → **220** pixels (`|dx ± dy| ≤ 7.2√2 = 10.18234`, and `dx ± dy` are integers, so the condition is `|s| ≤ 10 ∧ |t| ≤ 10 ∧ s + t odd`). The rotated covered set is asserted symmetric under `(dx, dy) → (dy, dx)`, which is true only if the aspect correction is applied — **this is the aspect gate**;
   - the same half-extents as an **ellipse**, rotation `0` → **164** pixels (`dx² + dy² ≤ 51.84`, counted per quadrant `7,7,7,6,6,5,3 = 41`). `(2i+1)² + (2j+1)² = 207.36` has no integer solution, so no pixel centre is on the boundary; the smallest interior margin is `1.34` px² (`(±5.5, ±4.5)`, `50.5` against `51.84`) and the smallest exterior margin `2.66` px² (`(±3.5, ±6.5)`, `54.5`) — both four orders of magnitude above f32 noise and recorded in the fixture;
   - the ellipse is asserted circular in pixels — by `|hw·a − hh| ≤ 4 ULP` in the fixture's f64 transcription and by exact `f32` equality on the shader-consumed constants (`hw·a` and `hh` are the same f32 bit pattern for `a ∈ {64/36, 16/9, 1920/1080}`; in f64 `0.1125 · 16/9 = 0.19999999999999998`, so exact f64 equality is not claimed) — and its bounding box is 14 × 14 pixels.
3. **Feather.** `feather = 4000` (`f = 0.4`): `w(D = 0.8) = 0.84375`, `w(D = 1.0) = 0.5`, `w(D = 1.2) = 0.15625` — exact in real arithmetic; in f32, `0.4` is not dyadic, so `1 ± f` each round and `w(1.2)` lands one ULP off (measured 1.2e-7) — the fixture asserts the first two bit-exact and the third within `1.5e-7`, and adds a dyadic `feather = 2500` case (`D = 0.875 / 1.0 / 1.125`) where all three anchors and the symmetry are bit-exact; the symmetry `w(1 − δ) + w(1 + δ) = 1` is asserted for δ ∈ {0.1, 0.2, 0.4}; the affected set is exactly `{D < 1.4}` and every pixel with `D ≥ 1.4` is bit-identical. `feather = 0` takes the hard branch and yields exact `0`/`1`.
4. **Combine.** Window A centred `2500/2500` (576 px) and window B at `center_x = 7500` (columns `x ∈ 32..=63`, 576 px), overlap columns `x ∈ 32..=47` → **288 px**. Union = **864**, intersection = **288**, both hand-derived by inclusion–exclusion and asserted on CPU and GPU. Per-window `invert` inside a union is asserted separately.
5. **Qualifier anchors**, computed from encoded triples and fed as `grade709_decode(e)`:
   - `e = (0.8, 0.2, 0.2)`: `C = 0.6`, `S = 0.75`, `Y = 0.32756`, `H = 0°`. With `hue_center = 0`, `hue_width = 3000`, softness 0 → `q = 1`;
   - **wraparound**: `e = (0.8, 0.2, 0.35)` → `H = 345°`. With `hue_center = 35000`, `hue_width = 1000`, softness 0 → `h = 1`. With `hue_center = 200` (2°), `hue_width = 1000`, `hue_softness = 1000` → `dh = 17°`, `t = 0.7`, `smoothstep = 0.784`, **`h = 0.216`**;
   - **achromatic**: `e = (0.5, 0.5, 0.5)`, `C = 0`. With `hue_width = 3000` → `h = 0` and `q = 0`. With `hue_width = 18000` → `h = 1` and `q = 1`. Both branches asserted;
   - **saturation softness**: `S = 0.75`, band `8000..10000`, softness `1000` → `min(smoothstep(0.7, 0.8, 0.75), 1) = min(0.5, 1) = 0.5`;
   - degenerate `sat_low > sat_high` → `0`, with `matte_band_inverted_by_automation` reported.
6. **Mix and invert.** `m_raw = 0.5`, `matte_mix = 6000` → `m = 0.3`, and `out = x + (node(x) − x)·0.3` asserted against the hand-computed node output on three raster samples. `matte_invert` on `m_raw = 0.15625` → `0.84375`. `matte_mix = 0` makes the node inactive with `MatteExcluded` and bit-identical to node removal.
7. **Keyframed window motion.** `matte_window0_center_x_basis_points` keyframed `Linear` from `2500` at frame 0 to `7500` at the last frame. At frame 0 the covered set is columns `x ∈ 0..=31` (576 px); at the last frame, `x ∈ 32..=63` (576 px); the two sets are asserted **disjoint**, and containment holds at each frame independently. A `Linear` keyframe on `matte_window0_shape_token` is rejected with `NonHoldKeyframeParameter`.
8. **CPU/GPU parity.** The CC1 §6.2 numbers reused verbatim and asserted equal to the code constants: monitor max ≤ 2, P99 ≤ 1, mean ≤ 0.50; neutral identity max ≤ 1, P99 ≤ 1, mean ≤ 0.25; linear (`|value| ≤ 1`) max ≤ 1.5e-3, P99 ≤ 7.5e-4, mean ≤ 2.5e-4; the `(1, 2]` band uses `9.765625e-4`; samples above 2 are excluded, counted, and recorded. Run on `cc5_parity_raster` with a windowed case, a qualifier case, a windowed + qualifier + feather case, and a full stack `[technical_lut, primary_correction(matte), color_wheels(matte), color_curves(matte), creative_look(matte)]`, on the software fallback by default and on hardware in the `--ignored` lane. No new tolerance is invented.
9. **Matte proof fidelity.** The §4.1 coverage image equals the CPU reference `round(255·m)` within **1 code** for feathered cases and **exactly** (0 codes) for every `feather = 0` case — a step function has no interpolation error, and §9.2's margins put every pixel far from the boundary. Coverage alpha is 255 everywhere. An inactive node and a matte-free node fail typed rather than returning a frame.
10. **Matte-scoped scopes and proof variants.** With the §9.1 window, a matte-scoped `get_video_scopes_v2` reports `transparent_pixel_count == 1728` and `visible == 576`, exactly the outside/inside counts; ROI ∩ matte is asserted on a half-frame ROI; `compare_scope_evidence` rejects a matte-scoped result against an unscoped one. `render_color_proof` with `matte_comparison: inside_only` and `outside_only` partitions the raster: with `feather = 0` and `mix = 10000`, every pixel differs from `before` in exactly one of the two variants.
11. **Tracked-shot proof.** A generated clip via `GeneratedMedia::ffmpeg`,
    640 × 360 / 25 fps, 100 frames: a **solid dark background**
    (`color=c=0x303030:s=640x360:r=25`) with an 80 × 80 white `color` source
    composited by `overlay` whose expressions are analytic —
    `x = 320 + 120·sin(2πt/8) − 40`, `y = 180 + 60·sin(2πt/8) − 40`
    (`overlay` exposes `t` as time and evaluates per frame; the pinned
    FFmpeg 8 `drawbox` filter exposes **no** time variable and a `t` in its
    expressions is read as the thickness sentinel, so the box silently never
    appears — measured, which is why `drawbox` is not used) — muxed with
    explicit `bt709` primaries/transfer/matrix and `tv` range plus the
    matching `-x264-params`, because CC1 rejects an untagged source. The
    background is solid (or `testsrc2` blurred and dimmed) on purpose: §5.2's
    box rule makes the SAD template window-sized, i.e. ~70 % static
    background around a 64-px subject, and a static high-contrast background
    pins the match at zero displacement with 0.86–0.99 confidence (measured:
    x froze at 5330 bp, y jumped to 4555 bp error on `testsrc2`; 25/43 bp on
    a solid background). The realised box snaps to even pixel offsets
    (`2·floor(edge/2)`, yuv420p chroma), so the analytic expectation uses
    that form or budgets ≤ 30 bp (x) / ≤ 55 bp (y) against the gate.
    `tracking_sample_frames(0..100, 5)` yields `0, 4, 9, …, 94, 99` (even
    intervals, not multiples of 5); expectations are evaluated at those
    frames. The fixture pins `step_frames = 5`, `search_radius_percent = 25`,
    and `max_width = 512` (thumbnail 512 × 288; at 256 one row is 69 bp and
    raw y error reaches 139 bp; at `step_frames = 10` the last-sample lag is
    576 bp against the 600 bp gate) and asserts the precondition
    `max_step_pixels ≤ radius_pixels` on both axes (measured 14.9/7.5 px vs
    128/72 px), recording `max_demanded_step_pixels` and
    `search_radius_pixels` in the manifest. The largest demanded step is
    292 bp (x) / 260 bp (y), inside `MATTE_TRACK_MAX_STEP_BASIS_POINTS = 800`.
    `MATTE_TRACK_TOLERANCE_BASIS_POINTS = 200` is asserted on the **raw
    observations** at every sample (measured max 25/43 bp); the smoothed
    `curves` are asserted against `MATTE_TRACK_SMOOTHED_TOLERANCE_BASIS_POINTS
    = 600` (measured max 302/278 bp, entirely the §5.2 last-sample median
    substitution). The tool converts thumbnail pixels with §5.2's
    `round((pixel + 0.5)·10000/extent)` — asserted explicitly, with its ≤ 10
    bp (x) / ≤ 17 bp (y) divergence from `pixel_to_basis_points`'s `extent −
    1` denominator recorded so a refactor cannot quietly swap them.
    Committing the plan onto a window of `half_width = 1300`,
    `half_height = 1800` then yields containment at every frame 0..99 of the
    linearly interpolated smoothed curve: every pixel of the analytic box has
    `m > 0`, with derived margins `1300 − 625 = 675` bp and
    `1800 − 1111 = 689` bp against a worst case of ≈ 332/333 bp including
    quantisation. The case is run at layer `scale ∈ {0.5, 1.0}` to assert
    the §5.2 coordinate conversion (at 0.5 the subject is 32 thumbnail px
    and raw y error rises to 121 bp — still inside the gate; the tool must
    state whether `box_percent` is rescaled by the layer scale on the
    composited thumbnail, and the fixture asserts that choice). A separate
    case forces low confidence and asserts `tracking_confidence_too_low`
    with `field`/`observed`/`allowed`.
    **Measured (2026-08-25, review erratum):** the media fixture runs the
    containment loop twice — on the analytic centres (worst margin 651.8 /
    647.1 bp at frame 50) and on a simulated tracker curve (analytic centres
    + deterministic ±200 bp raw jitter → `stabilize_tracked_centres_basis_points`
    with the tool's constants). The smoother's last-sample lag measured 491 bp
    (x) / 276 bp (y), so the lagged worst margins are 192.1 / 426.8 bp at
    frame 99: the 675/689 bp budget is consumed by raw tolerance *and* lag
    together, not by interpolation alone, and only ≈192 bp of the x budget
    survives. The agent-side e2e asserts containment of the *real* tool's
    smoothed curve. Also, the media fixture asserts the §5.2 offset leg
    directly: `y_percent = +20` at scale 1 moves the coverage box down by
    exactly 72 px of 360 and `x_percent = +20` right by exactly 128 px of 640.
12. **Migration and `mask` regression.** A CC4 project renders **bit-identically** after CC5 — asserted `to_bits`-identical in linear values and byte-identical in monitor RGBA8, which is what §2.5.4's mandatory skip buys. The existing `mask` fixture is unchanged. A clip carrying **both** a `mask` and a matte-carrying node is rendered through `render_working`: the layer's alpha equals the mask-only alpha byte-for-byte and its RGB equals the matte-only RGB byte-for-byte, proving the two never interact.
13. **Buffer and limits.** `GRADE_BUFFER_WORST_CASE_BYTES == 17_680`; `COMPOSITOR_REQUIRED_STORAGE_BUFFER_BINDING_SIZE == 32_768` and `>= GRADE_BUFFER_WORST_CASE_BYTES`; `COMPOSITOR_REQUIRED_STORAGE_BUFFERS_PER_SHADER_STAGE == 1`; `GRADE_ABI_VERSION == 3`; sixteen curve-plus-matte nodes serialize to exactly the worst case with correct, non-overlapping `payload_word_offset` and `v11` offsets; `v11 == 0` on every `technical_lut` record; the layer quad's pixel aspect equals the output raster aspect at three `scale` values.
14. **Serialization and history** (`tests/cc5_core.rs`). `AddEffect`, `SetEffectParam`, `SetEffectKeyframes`, `ClearEffectKeyframes` for all four kinds save/reopen, journal-replay, undo, and redo with values, window indices, and vector positions preserved. Rejections asserted atomically with `field`/`observed`/`allowed`: out-of-range centre, `half_width = 0`, `shape_token = 3`, `window_count = 5`, `matte_*` on `technical_lut` (`UnknownEffectParameter`), non-`Hold` keyframes on each Hold-only parameter, each `MatteExcluded` / matte-inactive integer test, and the `(matte_enabled = 0, matte_mix = 0)` case rendering unmasked at full strength.
15. **Agent plan-not-apply.** `plan_secondary_correction` returns exact operations, binds to the analyzed revision, fails closed on a stale revision, and leaves the source document byte-identical; its `predicted_coverage` matches `inspect_grade_matte` after the plan is applied. `inspect_grade_matte` and `render_color_proof` mutate nothing. `track_matte_window` commits nothing and its plan preview names both keyframe operations.
17. **Skin and product qualifier fixtures.** A synthetic chart raster carrying
    (a) four skin patches spanning light to deep and (b) two saturated product
    patches (a red and a cyan), each in a neutral surround. A qualifier tuned
    to one skin band selects every pixel of its patch and **exactly zero**
    pixels of the surround, the other patches, and the product patches; the
    surround and non-selected patches are `to_bits`-identical to the ungraded
    render; the alpha channel is byte-identical. The same is asserted for a
    product-hue qualifier. This is the roadmap's named exit evidence for
    workflow #5 and is not a skin-tone *quality* claim (CC6).
16. **Performance evidence.** The GPU render time of a 16-node × 4-window × qualifier stack at 1920 × 1080 is measured and recorded on both lanes, with a soft budget of one 24 fps frame (41.7 ms) on the hardware lane. Recorded evidence, not a hard gate — but a regression must be visible.

No tolerance may be used to excuse a changed pixel outside a matte, a modified alpha byte, an intermediate clamp, a wrong node order, or a matte applied without a position.

## 10. Implementation order

1. **Core descriptors and validation.** `crates/kinewright-core/src/multicam.rs`
   (promote and rename `stabilized_focus_values` → `stabilize_tracked_centres_basis_points`; no behaviour change); `crates/kinewright-core/src/effect.rs` (generated `matte_*` tables via a `matte_window_parameter_table!` macro, `MATTE_WINDOW_LIMIT = 4`, `MatteParams` / `ResolvedMatte` / `ResolvedMatteWindow`, `matte_inactive`, `ColorNodeInactiveReason::MatteExcluded`, matte-capable-kind predicate); `operation.rs` (rename `is_curve_point_count_parameter` → `is_hold_only_parameter` and extend it, degenerate-band reporting, new `OpError` variants); `crates/kinewright-core/tests/cc5_core.rs`.
2. **CPU reference math.** `crates/kinewright-media/src/color_pipeline.rs` (`MatteWindow`, `Matte`, `Matte::coverage`, `ColorNode` matte fields, `apply_color_nodes_at` replacing `apply_color_nodes`), written independently of the compositor.
3. **GPU ABI and shader.** `crates/kinewright-media/src/compositor.rs` (matte block builder, `raster_aspect` and `matte_debug_node` plumbing through `composite`, `v11` offsets, `header.w`, `GRADE_ABI_VERSION = 3`, new binding-size and worst-case constants, `render_matte` with the transfer-free readback); `compositor.wgsl` (`matte_window_weight`, `matte_qualifier_weight`, `matte_coverage`, `apply_color_nodes(rgb, uv)`, the debug early return).
4. **Proof and scopes plumbing.** `crates/kinewright-core/src/media.rs` (`MatteProof`, `MatteProofMetadata`, `Analysis::matte_proof_for_document`); `crates/kinewright-core/src/scopes.rs` (`MatteRegionDescription`, comparison equality); `crates/kinewright-media/src/engine.rs` (isolated matte proof); `render.rs` if the isolation helper is shared.
5. **Fixtures.** New `crates/kinewright-media/src/cc5_fixtures.rs`, registered in `lib.rs`, reusing `cc1_fixtures.rs` helpers for provenance, diff metrics, the banded linear gate, and evidence emission; `tests/fixtures/cc5_manifest.json`; the §9.11 generated clip through `test_support::GeneratedMedia`.
6. **Agent surface.** `crates/kinewright-agent/src/color_status.rs` (`plan_secondary_correction`, matte manifest, `RenderColorProofArgs.matte_comparison`); `color_scopes.rs` (`matte_region`); `server.rs` (`inspect_grade_matte`, `track_matte_window`, `excluded_effect: EffectId`); `schema.rs` (71 → 74, compact matte legend); `runtime.rs` (`CAPABILITY_KIND_OVERRIDES`).
7. **Human UI.** `crates/kinewright-app/src/inspector_ui.rs` (matte section, hidden raw parameters, keyframe badges and clear); new `matte_overlay_ui.rs` (overlay, hit-testing, `pub(crate) MatteProofSource` + `AnalysisMatteProofSource`); `preview_ui.rs` (`Sense::click_and_drag`, overlay, matte-view toggle); `color_scopes_ui.rs` (matte-scoped toggle, consuming `MatteProofSource`).
8. **Docs.** This file; `docs/ROADMAP-AND-WORKFLOWS.md` current-status bullets and the CC5 staged row; `CHANGELOG.md`.

Steps 1 → 2 → 3 are strictly ordered. Step 4 depends on 3. Step 5 depends on 3 and 4. Steps 6 and 7 depend on 1 and 4 and may proceed in parallel with 5.

## 11. Explicit deferrals

- **Spatial blur or softening of the matte.** A blur needs neighbouring pixels, which the per-pixel node loop does not have; it would require a separate matte pass, a matte texture, and its own sampling and edge-handling contract. Feather covers edge softness; a genuine blur is its own slice. This is why §2.2 has no `matte_blur_radius`.
- Polygon, bezier, freehand roto, and shape point animation.
- Per-edge or asymmetric (inner/outer) feather.
- Hue-vs-hue, hue-vs-saturation, saturation-vs-saturation, and luma-vs-saturation curves (deferred with CC3's list; they are a curve model, not a matte).
- Automatic subject, face, skin, or object detection, segmentation, and ML mattes. CC5 planners are request-driven and evidence-only.
- Planar, scale, or rotation tracking, occlusion handling, backward tracking, and tracker refinement UI.
- Manual keyframe authoring in the app, a timeline keyframe display, and a keyframe editor. CC5 adds indicators and a clear action only.
- Matte sharing between nodes, matte copy-paste, node groups, and still stores.
- More than four windows per matte, and mattes on `technical_lut`.
- Matte-scoped gamut, legal-range, and skin QC (CC6).
- Denoise, sharpen, or despill inside a matte.
- **Pre-existing M40 gap, found during the CC5 review and since closed:** `track_mask_region` and `track_reframe_subject` used to write tracked *composite*-space centres straight into the mask/reframe parameters without the §5.2 composite→layer conversion. Both now route through `LayerTransform` with the transform resolved per sample frame (see `M40-GENERALIZATION-GAUNTLET.md`, "Tracker coordinate space"); `track_matte_window` keeps its static-transform rule from §5.2.

CC5 is complete only when a colourist can draw a window or pull a qualifier on one correction node, see exactly which pixels it affects, feather and invert it, track it through a shot from an agent proposal, measure the graded result inside it with the ordinary scopes, undo any of it in one step — and prove that nothing outside the matte, and no alpha byte anywhere, moved.

## 12. Risks

- **Payload size and binding portability.** The buffer grows to 17,680 bytes (17.3 KiB) worst case and the binding requirement doubles to 32 KiB. Mitigation: keep the binding *count* at 1, raise and assert the constant in the limit-contract test, write no block for an inactive matte, and assert the exact worst case in §9.13.
- **Per-pixel cost.** Sixteen nodes × four windows × a qualifier is ~64 window evaluations and 16 `grade709` triples per pixel. Mitigation: the `v11 == 0` early-out, the loop bounded by `window_count`, the qualifier skipped when disabled, and §9.16's recorded budget so a regression is visible. If the qualifier's `pow` calls dominate, the encode is hoisted into a local per node — a measured change, not a speculative one.
- **Hue precision across CPU and GPU.** `H` involves a division by `C` that is ill-conditioned for near-achromatic pixels, and a `mod 6` whose branch differs by a ULP near a hue sector boundary. Mitigation: the branch order is normative and written out; the clamped local copy bounds the inputs to `[0, 1]`; the qualifier anchors sit far from sector boundaries; and near-achromatic pixels are dominated by the `C == 0` rule and the saturation band rather than by hue precision. A hue-sector-boundary sample is included in the parity raster and its divergence is recorded rather than assumed.
- **Tracker quality.** The existing matcher has no scale, rotation, or occlusion handling, so a subject that turns, is occluded, or changes size will drift. Mitigation: the confidence gate with a typed low-confidence failure and a manual fallback, the median filter and step limit from M40, the explicit `tracking_boundary` statement, and the fact that the plan is never committed without human or agent review.
- **Overlay UX and input regressions.** Giving the viewer `Sense::click_and_drag()` is the first interactive change to the preview. Mitigation: input is consumed only while a matte section is expanded for the selected clip; the overlay is a pure function of the document and the `image_rect` transform, so it is testable without a window; every gesture uses a coalesce key and §9's app tests assert one undo entry per gesture.
- **Two coverage concepts in one project.** A user who already knows the `mask` effect will reasonably expect a matte to change alpha. Mitigation: distinct names in the inspector, distinct reporting in the manifest, §9.12's regression proving they never interact, and the roadmap principle quoted in the UI copy.

---
