# CC3 curves and wheels

Status: implementation contract, 2026-08-24
Depends on: [CC0](ROADMAP-AND-WORKFLOWS.md), [CC1 managed SDR primary](CC1-MANAGED-SDR-PRIMARY.md), [CC2 scopes and matching](CC2-SCOPES-AND-MATCHING.md)
Scope: two new ordered, serializable colour-correction nodes — wheels and curves — inside the CC1 managed SDR pipeline.

CC3 does not change CC1's input, working, monitoring, or delivery contract. It adds
two node kinds to the correction stage that CC1 already declared ordered and
inspectable. Every CC1 invariant, especially the no-intermediate-clamp invariant
and the `clip.effects` execution order, is preserved verbatim.

The words **must**, **must not**, and **may** in this document are normative.

## 1. In scope and out of scope

CC3 delivers:

- one serializable `color_wheels` effect: ASC CDL-style slope/offset/power with
  per-channel and master integer controls;
- one serializable `color_curves` effect: master + red + green + blue monotone
  interpolated curves with integer point coordinates;
- an explicit, invertible grading encoding (`grade709`) used inside both nodes,
  with a defined sign-preserving extension over negatives and over-range values;
- an extension of the ordered node stack and its GPU storage-buffer ABI to carry
  a node-kind tag, a per-node bypass flag, and curve payloads;
- a per-node `bypass` control and a reset defined entirely in existing operations;
- a keyframing policy for scalar wheel controls and for curves;
- human wheel and curve-editor surfaces with coalesced, undoable commits;
- `plan_color_wheels` and `plan_color_curves` evidence-only, revision-gated agent
  planners, and manifest reporting of the new ordered stages; and
- identity, monotonicity, boundary, independence, ordering, parity, serialization,
  and plan-not-apply fixtures with numeric gates.

CC3 does **not** deliver HSL qualifiers, geometric windows, mattes, tracking, or
any node-scoped secondary (CC5); LUT/look management, project-owned hashed LUT
assets, shapers, or look mix (CC4); HDR, PQ/HLG, log-to-display transforms, camera
RAW, ACES/OCIO, gamut mapping, or delivery beyond the CC1 Rec.709 contract (CC6);
hue-vs-hue / hue-vs-sat / sat-vs-sat curves (CC5); automatic grading of any kind.
CC3 adds no `auto_grade` operation and no tool that mutates a document.

## 2. Node model

### 2.1 The `grade709` working encoding

Both CC3 nodes take a scene-linear working value, evaluate their controls on a
display-referenced encoding of that value, and return a scene-linear value. The
encoding exists so that "lift", "gamma", "gain", and curve coordinates behave the
way an SDR colourist expects: 0.0 is black, 1.0 is display white, and 18% scene
grey sits near the middle of a curve widget instead of in its bottom eighth.

Rejected alternative: evaluating slope/offset/power and curve points directly in
scene-linear light — rejected because a linear-light curve editor puts roughly 90%
of the usable adjustment into the bottom 20% of the widget, and a linear-light
`power` is a far stronger control than any published wheel contract.

The `grade709` pair is the BT.709 curve shape with the *precise* continuity
constants rather than the rounded broadcast constants, so that the pair is a
strict, C¹-continuous bijection on all of ℝ:

```text
ALPHA = 1.0992968          (f32; precise BT.709 alpha)
BETA  = 0.018053969        (f32; precise BT.709 beta)
BETA_E = 4.5 * BETA = 0.08124286
K     = ALPHA - 1 = 0.0992968
INV   = 2.2222223          (f32 nearest of 1/0.45)

E(x) = sgn(x) * 4.5 * |x|                            if |x| <  BETA
     = sgn(x) * (ALPHA * |x|^0.45 - K)               otherwise

D(e) = sgn(e) * |e| / 4.5                            if |e| <  BETA_E
     = sgn(e) * ((|e| + K) / ALPHA)^INV              otherwise
```

`sgn(0) = 0`. Implementations must not use `f32::signum`, which returns ±1 at
zero; WGSL `sign` already returns 0.

`E` and `D` are exact analytic inverses, strictly increasing, odd, and defined for
every finite input. `E(0) = 0`, `E(1) = 1`, `D(0) = 0`, `D(1) = 1`. No CC3 stage
clamps. `grade709` is an internal grading parameterization only. It **must not**
be used as a monitor or delivery transform, and it **must not** replace CC1's
`encode_bt709`/`decode_bt709`, whose rounded 1.099/0.018/0.081 constants remain
the normative source-decode and monitor-encode functions with their documented
seam. Two distinct functions are intentional: a monitor transform must match the
broadcast standard; a grading parameterization must be an exact bijection so that
the identity and monotonicity gates below are provable rather than approximate.

Worked anchors (derived by hand from the equations above and independently
re-derived during contract review; normative to ±2e-5):

| Input linear | Node and controls | Output linear |
| --- | --- | ---: |
| 0.18 | `E(0.18)` only | 0.408848 (grade709) |
| 0.18 | wheels, `gain_red_thousandths = 1200`, all else neutral (red channel) | 0.250771 |
| 0.18 | wheels, `lift_master_basis_points = -500`, `gamma_master_thousandths = 1200` | 0.100923 |
| 0.18 | curves, master points `(0,0) (5000,6000) (10000,10000)` | 0.262441 |

### 2.2 `color_wheels`

The canonical effect name is `color_wheels`. Its math is ASC CDL slope/offset/
power (SOP), evaluated per channel in `grade709`:

```text
e_c    = E(x_c)
slope_c  = (gain_c  / 1000) * (gain_master  / 1000)
offset_c = (lift_c + lift_master) / 10000
power_c  = (gamma_c / 1000) * (gamma_master / 1000)
y_c    = e_c * slope_c + offset_c
z_c    = sgn(y_c) * |y_c| ^ power_c
x'_c   = D(z_c)
```

for `c` in `{red, green, blue}`. Master combines multiplicatively for gain and
power (exponents compose by multiplication, so `(x^a)^b = x^(a·b)` is exact) and
additively for lift.

Chosen over shadows/midtones/highlights naming because CC1 already owns
`blacks_percent`, `shadows_percent`, `highlights_percent`, and `whites_percent`
as additive linear-light lifts; a second control set with the same words and
different semantics would be unreadable in a proof manifest. Rejected alternative:
classic display-referred lift `x·(1-l)+l` — rejected because its pivot behaviour
is not standardized and could not be stated as a cross-implementation contract.

Deviation from ASC CDL v1.2, stated explicitly: the standard clamps `y_c` to
`[0, 1]` before the power step. CC3 does not. It uses the odd extension
`sgn(y)·|y|^p` instead, because CC1 guarantees that recoverable undershoot and
over-range highlights survive every correction stage. `power_c` is always
strictly positive (minimum `0.1 · 0.1 = 0.01`), so `|0|^p = 0` and no NaN is
produced. `slope_c` may be exactly `0` at the minimum bound; the node is then a
constant per channel, which is monotone non-decreasing but not strictly
increasing. That is a legal boundary state, not an error.

### 2.3 `color_curves`

The canonical effect name is `color_curves`. It holds four curves — `red`,
`green`, `blue`, `master` — evaluated in `grade709`:

```text
e   = (E(x_r), E(x_g), E(x_b))
e_r = curve_red(e_r); e_g = curve_green(e_g); e_b = curve_blue(e_b)
e_c = curve_master(e_c)   for each c, using the same curve
x'  = (D(e_r), D(e_g), D(e_b))
```

Per-channel curves run first, then the master curve is applied identically to all
three channels. The fourth curve is named `master`, not `luma`, and it is defined
as applied identically per channel. Rejected alternative: a true Rec.709 luma
curve with chroma re-scaled by `y'/y` — rejected because the ratio is undefined at
`y = 0` and destroys the per-channel monotonicity guarantee.

**Point representation.** A curve is an ordered list of 2..=16 points. Each
coordinate is an integer in basis points of the `grade709` range: `10000` is
display white, `0` is black, and the inclusive bound `-2000..=12000` lets a point
sit below black or above white so that over-range material can be shaped rather
than clipped. This is the same unit CC1 already uses for
`contrast_pivot_basis_points`.

**Sorting and uniqueness.** `x` must be strictly increasing over the active
prefix. Equal or descending `x` is rejected by validation. `y` is unconstrained
apart from its bounds; a non-monotone `y` sequence is a legal creative curve.

**Interpolation.** Monotone cubic Hermite with Fritsch–Carlson tangent limiting.
Chosen because it is deterministic, has no free parameters, is stable in f32, and
guarantees that a monotone point sequence yields a monotone curve — which is
exactly the CC3 exit gate. Rejected alternative: Catmull–Rom with a monotonicity
clamp — rejected because the clamp is applied after the fact and its result
depends on clamp ordering, so two independent implementations can disagree.

Tangents are solved once per curve, on the host, in a single forward pass. Let
`n` be the point count, with points in `grade709` units (basis points divided by
10000):

```text
1. delta[i] = (y[i+1] - y[i]) / (x[i+1] - x[i])      for i in 0..n-2
2. m[0]   = delta[0]
   m[n-1] = delta[n-2]
   m[i]   = (delta[i-1] + delta[i]) / 2               for 0 < i < n-1
3. for i = 0, 1, ... n-2, in increasing order, mutating m in place:
     if delta[i] == 0.0:
         m[i] = 0.0; m[i+1] = 0.0
     else:
         a = m[i]   / delta[i]
         b = m[i+1] / delta[i]
         if a < 0.0: m[i]   = 0.0
         if b < 0.0: m[i+1] = 0.0
         if a >= 0.0 and b >= 0.0 and a*a + b*b > 9.0:
             tau    = 3.0 / sqrt(a*a + b*b)
             m[i]   = tau * a * delta[i]
             m[i+1] = tau * b * delta[i]
```

The forward, in-place ordering of step 3 is part of the contract; a different
visitation order produces different tangents for some inputs. Both
implementations treat a non-positive `x` span in step 1 as a zero secant
rather than dividing: Core guarantees strictly increasing `x` for every
resolved curve (§3.4), so the branch is unreachable in production, and it exists
only so that a hand-built point list can never produce an infinite or NaN
tangent that would poison the GPU buffer. Tangents are
dimensionless (`dy/dx`), so they are identical whether the points are expressed
in basis points or in `grade709` units.

**Evaluation.** For `x` inside segment `i` (`x[i] <= x < x[i+1]`), with
`h = x[i+1] - x[i]` and `t = (x - x[i]) / h`:

```text
t2  = t * t
t3  = t2 * t
h00 =  2*t3 - 3*t2 + 1
h10 =      t3 - 2*t2 + t
h01 = -2*t3 + 3*t2
h11 =      t3 -   t2
y   = h00*y[i] + h10*h*m[i] + h01*y[i+1] + h11*h*m[i+1]
```

**Extrapolation.** Linear, using the limited end tangents:

```text
x <  x[0]   ->  y = y[0]   + m[0]   * (x - x[0])
x >= x[n-1] ->  y = y[n-1] + m[n-1] * (x - x[n-1])
```

This keeps over-range values alive. Rejected alternative: constant clamping
outside the point domain — rejected because it silently clips over-range
highlights and violates CC1 §2.2 invariant 5.

**Identity.** A curve is *structurally identity* when it has exactly two points,
`(0, 0)` and `(10000, 10000)`. A curve whose points all lie on the diagonal is
*mathematically identity*: the tangents are all `1.0` and the Hermite basis
reproduces the line exactly. Only structural identity triggers the bit-identity
short-circuit in §3.3.

### 2.4 Serialization

Curves are serialized as ordinary integer effect parameters. No `ParamValue`
variant is added.

Rejected alternative: `ParamValue::Points(Vec<[i32;2]>)` — rejected because
`EffectParameterDescriptor` describes exactly one `(min, max, neutral)` integer,
`Keyframe.value` is a single `i64`, and every exhaustive `ParamValue` match in
core, media, app, and agent would change to gain nothing that the integer encoding
does not already provide (validation, bounds, undo, journal, keyframes, and
revision-gated agent ops all work unchanged).

Because an omitted parameter resolves to its neutral, the human and agent
workflows insert only `{curve}_point_count` and the coordinates of active points
when creating or editing a `color_curves` node. Points at index `>= point_count`
**should** be omitted from the stored parameter map so project JSON stays
compact; a stored value there is legal, ignored by rendering, and preserved
byte-for-byte.

Two mechanical accommodations are required in Core:

1. Add one `EffectUniform::ColorNode` variant, shared by every CC3 parameter and
   meaning "consumed by the ordered colour-node storage buffer, never by the
   `LayerParams` uniform block". `params_for` gains one match arm, not 146.
2. `crates/kinewright-agent/src/schema.rs` currently expands every descriptor
   parameter into the tool-description string. It **must** special-case
   `color_curves` and emit a compact pattern description (names, index range,
   bounds, neutrals) instead of 133 enumerated entries; otherwise every tool
   listing grows by several thousand tokens and violates M36's runtime-efficiency
   posture.

## 3. Canonical order and GPU mechanism

### 3.1 Canonical order

CC1 §3's pipeline is unchanged except that one line broadens:

```text
  -> managed colour-correction nodes, in serialized clip.effects order
     (primary_correction | color_wheels | color_curves)
```

Normatively: `primary_correction`, `color_wheels`, and `color_curves` form **one**
ordered node stack executed in `clip.effects` vector order. There is no fixed
inter-kind precedence: a project may place curves before wheels or wheels before
curves, and the two orders produce different, correct results. Each node is
self-contained — it consumes scene-linear working RGB and produces scene-linear
working RGB — and no RGB clamp occurs between nodes. The renderer must not
flatten, reorder, or merge nodes. A layer carries at most 16 colour nodes; more
is a typed `too_many_color_nodes` error, not a silent truncation.

### 3.2 GPU mechanism

Both nodes fit the existing per-node storage-buffer loop, extended with a kind
tag and a payload region. One storage buffer is retained;
`COMPOSITOR_REQUIRED_STORAGE_BUFFERS_PER_SHADER_STAGE` stays `1`, because a
second fragment-stage storage binding is not available on every supported
downlevel backend.

```wgsl
struct GradeBuffer {
    // x = active node count, y = payload word offset, z = ABI version (1), w = 0
    header: vec4<u32>,
    words:  array<f32>,
};
```

- Node records begin at word 4, stride 16 words (64 bytes):
  `[kind, payload_word_offset, bypass, reserved, v0 .. v11]`.
  `kind` and `payload_word_offset` are stored as `f32` and read with
  `u32(round(w))`; `bypass` is `0.0` or `1.0`.
  `kind` is `1.0` = primary, `2.0` = wheels, `3.0` = curves.
- Primary uses `v0..v9` exactly as today. Wheels uses
  `v0..v2 = slope_rgb`, `v3..v5 = offset_rgb`, `v6..v8 = power_rgb`.
- Curve payloads follow the node array. Each curve node owns 4 slots of 49 words:
  `[count, x0, y0, m0, x1, y1, m1, ... x15, y15, m15]`, ordered red, green, blue,
  master. Coordinates are in `grade709` units; tangents are dimensionless.
- Worst case: `16 + 16*64 + 16*4*49*4 = 13584` bytes.
  `COMPOSITOR_REQUIRED_STORAGE_BUFFER_BINDING_SIZE` becomes `16384`.
- Offset convention: every stored word offset (`header.y` and each record's
  `payload_word_offset`) is an index into `words`, i.e. the absolute word index
  minus the four header words, so `curve_eval(base, x)` reads
  `words[base]` directly. Therefore `header.y == 16 * active_node_count` and
  the first payload byte is `16 + 64 * active_node_count`.
- An empty stack (no active node) is written as the 16-byte header with
  `count = 0` plus one all-zero 64-byte record (80 bytes total) so the
  runtime-sized binding stays valid; the shader loops zero times.
- More than 16 managed nodes on one layer is rejected by Core at edit time
  (`TooManyColorNodes`); the serializer re-checks defensively and fails with a
  message prefixed `too_many_color_nodes:`.

**Tangents are solved on the host, not per pixel, and not baked into a LUT.**
Rejected alternative: baking each curve to a fixed-size f32 LUT — rejected
because it introduces a second normative specification (sample positions plus a
resampling rule) and an approximation error on top of a formula that a shader can
evaluate exactly in a bounded 15-iteration loop. Rejected alternative: solving
tangents per pixel in the shader — identical results, strictly more ALU.

The compositor's host-side tangent solve is *production* code. The CPU reference
in `color_pipeline.rs` **must** implement the §2.3 algorithm independently and
**must not** call the compositor's solver, so parity fixtures compare two
implementations of the written contract rather than one implementation with
itself.

Shader evaluation, WGSL-compatible and normative:

```wgsl
fn grade709_encode(v: f32) -> f32 {
    let s = sign(v); let a = abs(v);
    if a < 0.018053969 { return s * 4.5 * a; }
    return s * (1.0992968 * pow(a, 0.45) - 0.0992968);
}

fn grade709_decode(v: f32) -> f32 {
    let s = sign(v); let a = abs(v);
    if a < 0.08124286 { return s * a / 4.5; }
    return s * pow((a + 0.0992968) / 1.0992968, 2.2222223);
}

fn curve_eval(base: u32, x: f32) -> f32 {
    let count = u32(round(grade_buffer.words[base]));
    if count < 2u { return x; }
    let first = base + 1u;
    let last  = base + 1u + (count - 1u) * 3u;
    if x <  grade_buffer.words[first] {
        return grade_buffer.words[first + 1u]
             + grade_buffer.words[first + 2u] * (x - grade_buffer.words[first]);
    }
    if x >= grade_buffer.words[last] {
        return grade_buffer.words[last + 1u]
             + grade_buffer.words[last + 2u] * (x - grade_buffer.words[last]);
    }
    var segment = 0u;
    for (var i = 0u; i + 1u < count; i = i + 1u) {
        let xi = grade_buffer.words[base + 1u + i * 3u];
        let xn = grade_buffer.words[base + 1u + (i + 1u) * 3u];
        if x >= xi && x < xn { segment = i; }
    }
    let o0 = base + 1u + segment * 3u;
    let o1 = o0 + 3u;
    let x0 = grade_buffer.words[o0];      let y0 = grade_buffer.words[o0 + 1u];
    let m0 = grade_buffer.words[o0 + 2u]; let x1 = grade_buffer.words[o1];
    let y1 = grade_buffer.words[o1 + 1u]; let m1 = grade_buffer.words[o1 + 2u];
    let h = x1 - x0;
    let t = (x - x0) / h;
    let t2 = t * t; let t3 = t2 * t;
    return (2.0*t3 - 3.0*t2 + 1.0) * y0
         + (t3 - 2.0*t2 + t) * h * m0
         + (-2.0*t3 + 3.0*t2) * y1
         + (t3 - t2) * h * m1;
}
```

Bit-exact CPU/GPU agreement is not required; the CC1 §6.2 tolerances govern.

### 3.3 Inactive nodes

At every rendered frame, keyframes are resolved first (`Effect::evaluated_at`),
then a node is **inactive** when either

- its evaluated `bypass` is `>= 1`; or
- it is *neutral*: for `color_wheels`, all twelve controls equal their descriptor
  neutrals; for `color_curves`, all four curves are structurally identity
  (§2.3).

An inactive node is the exact identity function. It **must not** be written to
the GPU buffer and **must** be skipped by the CPU reference. Neutrality is tested
on the stored integers, never on floats. This is what makes the §10 identity gate
bit-identical rather than tolerance-bounded: a neutral node is not evaluated at
all, so no `E`/`D` round-trip error can appear.

### 3.4 Degenerate resolved curves

Keyframe evaluation can produce a point list whose active prefix is not strictly
increasing. Rendering a legal document must not fail. Normatively: the resolved
curve is truncated to the longest prefix `p` with strictly increasing `x`; if
`p < 2`, the curve is identity. The inspector reports
`curve_truncated_by_automation` for that node. No clamping, no reordering, no
error.

## 4. Controls

All parameters are `ParamValue::Integer`. Bounds are inclusive. An omitted
parameter resolves to its neutral. `uniform` is `EffectUniform::ColorNode` for
every row.

### 4.1 `color_wheels`

| Parameter | Stored unit | Min | Max | Neutral | Meaning |
| --- | --- | ---: | ---: | ---: | --- |
| `lift_master_basis_points` | 1/10000 of grade709 range | -2000 | 2000 | 0 | Offset added to all channels. |
| `lift_red_basis_points` | 1/10000 of grade709 range | -2000 | 2000 | 0 | Offset added to red, summed with master. |
| `lift_green_basis_points` | 1/10000 of grade709 range | -2000 | 2000 | 0 | Offset added to green, summed with master. |
| `lift_blue_basis_points` | 1/10000 of grade709 range | -2000 | 2000 | 0 | Offset added to blue, summed with master. |
| `gamma_master_thousandths` | 1/1000 exponent | 100 | 4000 | 1000 | Power applied to all channels. |
| `gamma_red_thousandths` | 1/1000 exponent | 100 | 4000 | 1000 | Red power, multiplied by master. |
| `gamma_green_thousandths` | 1/1000 exponent | 100 | 4000 | 1000 | Green power, multiplied by master. |
| `gamma_blue_thousandths` | 1/1000 exponent | 100 | 4000 | 1000 | Blue power, multiplied by master. |
| `gain_master_thousandths` | 1/1000 slope | 0 | 4000 | 1000 | Slope applied to all channels. |
| `gain_red_thousandths` | 1/1000 slope | 0 | 4000 | 1000 | Red slope, multiplied by master. |
| `gain_green_thousandths` | 1/1000 slope | 0 | 4000 | 1000 | Green slope, multiplied by master. |
| `gain_blue_thousandths` | 1/1000 slope | 0 | 4000 | 1000 | Blue slope, multiplied by master. |
| `bypass` | boolean token | 0 | 1 | 0 | `1` makes the node the identity. |

Combined bounds are therefore `slope ∈ [0, 16]`, `offset ∈ [-0.4, 0.4]`, and
`power ∈ [0.01, 16]`. Every bound is finite, and no combination produces NaN for
a finite input. Finite *output* is guaranteed for every §10.2 raster sample when
at most one of `slope` or `power` sits at its maximum; the simultaneous extreme
(`slope = 16` and `power = 16`) on linear `4.0` is mathematically ≈ 1.1e53 and
overflows f32 to `+inf` on the CPU reference, which the final monitor clamp
resolves to code 255. The GPU working surface is `Rgba16Float`: a hardware
adapter saturates an out-of-range f32 store to ±65504 and maps a true f32
infinity to NaN, which the monitor encode resolves to code 0 (measured on the
RTX 3080 Vulkan lane). That divergence is confined to this documented extreme,
which is therefore excluded from the parity raster; the boundary fixture
asserts the CPU `+inf`, asserts the GPU value is at or beyond the half-float
limit (no early clamp), and asserts the GPU monitor code is a clamp extreme
(0 or 255), never a mid-range value. Implementations must not clamp early to
avoid it.

### 4.2 `color_curves`

133 parameters, generated from three patterns. `{curve}` ∈
`{master, red, green, blue}`; `{j}` ∈ `0..=15`.

| Parameter | Stored unit | Min | Max | Neutral | Meaning |
| --- | --- | ---: | ---: | ---: | --- |
| `{curve}_point_count` | count | 2 | 16 | 2 | Active point count for that curve. |
| `{curve}_x{j}` | 1/10000 of grade709 range | -2000 | 12000 | `0` if `j == 0`, else `10000` | Input coordinate of point `j`. |
| `{curve}_y{j}` | 1/10000 of grade709 range | -2000 | 12000 | `0` if `j == 0`, else `10000` | Output coordinate of point `j`. |
| `bypass` | boolean token | 0 | 1 | 0 | `1` makes the node the identity. |

With every parameter at neutral, each curve is `(0,0) (10000,10000)` — the
structural identity. Points at index `>= point_count` are ignored; their neutral
values deliberately collide at `(10000, 10000)` and are not subject to the
strictly-increasing rule.

`AddEffect` and `SetEffectParam` validation additionally enforces, for each
curve independently: `x[i] < x[i+1]` for `i` in `0..point_count-1`. A violation is
`OpError::InvalidCurvePoints { effect, curve, index, previous_x, x }` and is
rejected atomically. `bypass` is a boolean token, not `ParamValue::Boolean`:
`validate_described_effect_parameter` accepts only `ParamValue::Integer`, and
`mask.invert` already establishes the 0/1 token precedent. Making `Boolean` valid
for descriptor parameters would require a second bound/neutral concept for no
gain.

## 5. Reset and bypass

**Bypass** is an ordinary serialized, undoable, keyframe-able integer parameter
per node. It is set with `SetEffectParam`; no new operation kind exists. A
bypassed node remains in `clip.effects` at its position, keeps all its values, and
is reported in every manifest with `"bypass": 1, "active": false` plus the reason
(`bypassed` or `neutral`). A bypass must never be implemented by removing the
effect, by zeroing controls, or by a UI-only flag.

**Reset** of a node is the existing pattern in
`crates/kinewright-app/src/inspector_ui.rs::color_node_reset_operations` (the
generalized successor of the CC1 primary reset): one `SetEffectParam` per
descriptor parameter set to its neutral, plus `ClearEffectKeyframes` for each
parameter that has automation, emitted as one batch. Curve reset emits the same
ops restricted to one curve's 33 parameters. Because Core validates strict `x`
against every intermediate document, a curve reset must order its coordinate
writes the same way §8's planner does (collapse the count, move points 0 and 1
in the legal order, write inactive indices, restore the count); a
descriptor-order batch is rejected whenever the stored `x1 <= 0`. No new
operation kinds are introduced by CC3.

## 6. Keyframing

Scalar wheel controls (`lift_*`, `gamma_*`, `gain_*`) and `bypass` are ordinary
integer keyframe parameters under the existing `SetEffectKeyframes` /
`ClearEffectKeyframes` rules. Authors should use `Hold` on `bypass`; `Linear` is
legal and resolves through the `>= 1` test.

Curves may be keyframed under exactly two policies, both expressible in the
existing model and enforced in `validate_curve`:

1. **Whole-curve steps.** `{curve}_point_count` and every `{curve}_x{j}` /
   `{curve}_y{j}` use `KeyframeInterpolation::Hold`. The curve switches
   discontinuously at each keyframe.
2. **Point-wise interpolation at constant point count.** Coordinates may use any
   interpolation, provided `{curve}_point_count` has zero or one keyframe over the
   clip.

Enforcement: `{curve}_point_count` accepts only `Hold` keyframes
(`OpError::NonHoldKeyframeParameter { effect, name }`), and if any coordinate of a
curve is keyframed while that curve's `point_count` has more than one keyframe,
the operation is rejected with
`OpError::CurvePointCountAnimatedWithPoints { effect, curve }`. Resolved curves
that still violate strict `x` ordering are handled by the §3.4 truncation rule.

## 7. Human UI

**Wheels.** Three trackballs — Lift (shadows), Gamma (midtones), Gain
(highlights) — each with a master ring or adjacent slider. The trackball position
`(u, v)` in the unit disc maps to per-channel integer deltas with primary
directions R at 90°, G at 210°, B at 330°:

```text
delta_c = round_half_away_from_zero(k * (u*cos(theta_c) + v*sin(theta_c)))
```

`k` is the control's maximum magnitude (2000 for lift, 1000 for gamma/gain
deviation from neutral). Results are clamped to the descriptor bounds. Ring or
slider drag drives the corresponding `*_master_*` control. Double-click on a ball
resets that wheel's four parameters. Numeric read-outs show `+0.0500`, `1.200`,
`1.200` for lift/gamma/gain respectively so the value is inspectable and matches
the stored integer exactly.

**Curve editor.** A square widget with the identity diagonal always drawn, a
channel selector (Master / R / G / B), the current curve, and its control points.
Click on empty space adds a point (rejected at 16); drag moves a point, clamped
to `-2000..=12000` on both axes and to at least 1 basis point of separation from
its neighbours in `x`; right-click or Delete removes a point (rejected at 2);
double-click resets the selected curve. The widget writes only integers.

**Commit and undo.** Every drag gesture on a wheel or a curve point applies live
operations so preview updates continuously, and the whole gesture collapses to a
single undo step through the Core actor's coalesced batch command (the same
mechanism the CC1 primary sliders use after the 2026-08-24 review fixes): live
operations during the drag carry a gesture key so the actor replaces the top
history entry instead of pushing one per frame, and the release sends the final
un-keyed commit. Producing dozens of undo entries per gesture is a defect, not an
acceptable fallback.

**Inspector state.** Each colour node card shows its stage index, a bypass
toggle, a per-node Reset, a keyframe indicator per control, the
`curve_truncated_by_automation` warning when applicable, and, when a control is
keyframed, a note that direct editing writes the static value rather than a
keyframe.

## 8. Agent surface

`plan_primary_correction` is unchanged. Two planners are added, both modelled on
it exactly: revision-gated, evidence-only, returning exact operations that the
caller must submit through the ordinary edit plan path. Neither applies
anything. A new node is created by one `AddEffect` whose parameter map carries
only the requested non-neutral values (no redundant trailing `SetEffectParam`);
an existing node is edited with `SetEffectParam` only. For curves the emitted
`SetEffectParam` order is itself part of the contract, because Core validates
strict `x` ordering against every intermediate document: the planner collapses
`{curve}_point_count` to 2, moves points 0 and 1 in whichever of the two orders
is legal, writes points at index `>= 2` while they are inactive, then restores
the count. `plan_color_curves` takes `bypass` as its own argument because it has
no free-form parameter map.

- `plan_color_wheels` — arguments: `expected_revision`, `clip_id`, optional
  `profile_assumption`, and a `parameters` map of the §4.1 names to integers.
  Returns `expected_revision`, `clip_id`, `effect_id`, `source_profile`,
  `profile_assumption`, `requested_parameters`, `resolved_parameters` (neutrals
  merged with requests), `operations`, and `existing_color_node_count`.
- `plan_color_curves` — same shape, but accepts an ergonomic
  `curves: { master?: [[x,y], ...], red?: ..., green?: ..., blue?: ... }` request
  and expands it to `{curve}_point_count` plus coordinate operations. The
  document remains integer-scalar. Point lists are validated for count, bounds,
  and strict `x` ordering before any operation is constructed; a violation
  reports field, observed value, and allowed values.

Both planners follow the CC2 review rule for existing nodes: when the clip
already carries a node of the requested kind, the plan targets that node with
`SetEffectParam` and reports `target_effect_id` instead of stacking a second
node, unless the caller passes `append: true`.

`INSPECTOR_TOOL_NAMES` grows from 64 to 66. Errors follow the CC1/CC2 pattern:
`field`, `observed`, `allowed`, `recovery_action`.

`get_color_context` clip status replaces its `primary_nodes` array with an
ordered `color_nodes` array. Each entry reports `stage_index` (position in
`clip.effects`), `effect_id`, `kind`, `bypass`, `active`, `inactive_reason`,
fully resolved parameters, and — for curves — the resolved point lists per curve.
`render_color_proof` manifests carry the same ordered stage list alongside the
existing source/working/monitoring/delivery descriptions and render-kind
provenance. Legacy stage warnings are unchanged.

## 9. Migration

There is no legacy equivalent of either node, so there is nothing to translate:

1. Pre-CC3 projects load unchanged. No effect name is canonicalized, no parameter
   is renamed, and no node is inserted.
2. A project without `color_wheels` or `color_curves` renders bit-identically to
   its pre-CC3 result, because inactive nodes are not evaluated (§3.3) and no
   existing node's math changed.
3. Save/reopen, journal replay, undo, and redo preserve CC3 nodes, their vector
   position, their parameters, and their keyframes byte-for-byte apart from
   documented JSON defaults.

`ColorPipelineState` stays `managed_sdr_v1`. Rejected alternative:
`managed_sdr_v2` — rejected because `pipeline_state` describes the
source→working→monitoring→delivery managed contract, not the inventory of
correction nodes; CC3 adds nodes inside a stage CC1 already declared ordered and
extensible, changes no colour description, and a bump would immediately fail
`delivery.rs`'s managed-delivery check for every existing CC1 project with no
semantic gain. A bump is warranted only when the working, monitoring, or delivery
contract itself changes — for example HDR.

## 10. Exit fixtures and numeric gates

The gate is a fixture suite in the style of
`crates/kinewright-media/src/cc1_fixtures.rs`, recorded as `cc3_fixtures.rs`.
Every fixture records the git revision, backend, adapter, software-fallback and
GPU-claim flags, OS, source profile, node stack, resolved parameters, and output
hashes.

### 10.1 Fixture-quality rules (from the CC1/CC2 review — normative)

1. Expected values are written out analytically from the §2 equations, either as
   literal constants in this document or transcribed independently in f64 in the
   fixture. A fixture **must not** obtain an expected value by calling
   `ColorWheels::apply`, `ColorCurve::evaluate`, the compositor, or the shader.
2. Every control at minimum, maximum, and a representative interior value has a
   numeric expected value. `is_finite()` alone is never a sufficient assertion.
3. Parity rasters must contain samples that exercise every control. CC1's raster
   contained no sample above 0.2 linear, which made highlights and whites parity
   vacuous. The CC3 raster is asserted, in the fixture, to span negatives, the
   0..1 range, and values above 1.
4. Manifest tolerances are asserted equal to the code constants
   (`MONITOR_CPU_GPU_MAX`, `LINEAR_CPU_GPU_P99`, …), not restated as literals.
5. GPU fixtures run on a hardware adapter when no software fallback is present,
   recording honest provenance, instead of panicking or silently claiming GPU
   coverage. The software-fallback lane and the hardware lane stay distinct.
6. Error assertions check `field`, `observed`, and `allowed`, not just the error
   variant or the field name.

### 10.2 The CC3 parity raster

`cc3_parity_raster()` is 24 linear levels

```text
-0.50, -0.25, -0.10, -0.02, -0.005, 0.0, 0.002, 0.005, 0.018053969,
0.03, 0.06, 0.10, 0.18, 0.25, 0.35, 0.50, 0.65, 0.80, 0.90, 1.00,
1.20, 1.50, 2.50, 4.00
```

crossed with 8 channel patterns (neutral, R, G, B, C, M, Y, and the skewed
`(L, L/2, L/4)`), giving 192 samples. The fixture asserts its own coverage,
counted over the **distinct linear channel values** present in the raster (the
skewed pattern contributes `L/2` and `L/4`): minimum value `<= -0.25`, maximum
`>= 4.0`, at least 5 negative values, at least 6 values above 1.0, at least 8
values in the closed interval `[0.5, 1.0]`, and at least 7 in the half-open
`(0.5, 1.0]`. (Counted over the 24 levels alone the list has only 4 above 1.0
and 4 in `(0.5, 1.0]`; the first draft of this contract overstated that.) A
raster that fails coverage fails the suite. The documented `slope = 16 ∧
power = 16` extreme is deliberately excluded from the parity raster because it
is non-finite by design (see §4.1); it has its own boundary fixture.

### 10.3 Required fixtures

1. **Identity.** A neutral `color_wheels` node, a neutral `color_curves` node,
   and a bypassed non-neutral node of each kind each produce output *bit-identical*
   to the same stack with the node removed, on the CPU reference and on the GPU,
   in both linear working values and monitor RGBA8.
2. **Encoding bijection.** For every raster sample, `D(E(x)) == x` within
   `LINEAR_CPU_GPU_MAX`; `E` is strictly increasing over the raster; the anchors
   in §2.1 hold to ±2e-5.
3. **Monotonicity.** A monotone-point curve (points sorted increasing in both
   axes, including a case with a zero-slope plateau) applied to the 8-bit and
   10-bit neutral ramps produces zero descending adjacent pairs after final
   monitor encoding. Same for wheels at every boundary combination with
   `slope > 0`.
4. **Boundary.** Every §4 control at minimum, maximum, and one interior value,
   each with a written-out expected value on at least three raster samples
   (a negative, 0.18, and 2.5). Output is finite on the whole §10.2 raster for
   each control at its bound individually, never NaN for any combination
   (the documented `slope = 16, power = 16` f32 overflow to `+inf` is asserted
   as `+inf`, not excused), over-range input survives, and a curve whose points
   sit at `-2000` and `12000` on the diagonal is identity within
   `LINEAR_CPU_GPU_MAX`. `gain_* = 0` produces the documented constant, not an
   error.
5. **Per-channel independence.** Between two *active* nodes that differ only in
   `*_red_*` controls or only in the red curve, green and blue outputs (and
   their monitor codes) are bit-identical on CPU and GPU. (Comparing against a
   node-removed baseline would be wrong: removing the node also removes the
   `E`/`D` round trip on green and blue.)
6. **Collinear identity.** A 16-point curve with all points on the diagonal is
   identity within `LINEAR_CPU_GPU_MAX` (it does not take the §3.3 short-circuit).
7. **Ordering.** `[wheels, curves]` and `[curves, wheels]` with the same non-neutral
   values produce different results, and each matches the CPU reference evaluated
   in the same vector order. A three-node stack `[primary, wheels, curves]` is
   also checked.
8. **Degenerate automation.** The §3.4 truncation rule is exercised with a
   keyframed `point_count` and coordinates that cross, on CPU and GPU.
9. **CPU/GPU parity.** The CC1 §6.2 numbers are reused verbatim and asserted equal
   to the code constants: monitor max `<= 2`, P99 `<= 1`, mean `<= 0.50`; neutral
   identity max `<= 1`, P99 `<= 1`, mean `<= 0.25`; linear (on samples with
   `|value| <= 2`) max `<= 1.5e-3`, P99 `<= 7.5e-4`, mean `<= 2.5e-4`. Samples with
   `|linear| > 2` are excluded from the linear numeric gate exactly as CC1 does,
   with the excluded count recorded, and remain subject to the monitor-code,
   finiteness, and monotonicity gates. Run on the software fallback by default and
   on a hardware adapter in the explicit lane. No new tolerance is invented.
   §2.2 is deliberately not Lipschitz at `y = 0`: for `power < 1` the derivative
   of `sgn(y)·|y|^power` is unbounded there, so an input perturbation of
   `3.05e-5` in a channel whose graded value should be exactly zero produces
   `0.18` in linear light (105 monitor codes) at `gamma_master_thousandths =
   100`. The first lavapipe run (2026-08-24) found exactly that, caused by
   bilinear sub-texel leakage rather than by the maths. The parity gate
   therefore depends on the CC1 §6.2 pixel-exact sampling clause rather than on
   tolerance width, and no epsilon guard is added to `sgn(y)·|y|^power`: the
   function is evaluated as written, and it is the renderer's obligation to
   feed it the texel the CPU reference was given.
10. **Serialization and history.** Save/reopen, journal replay, undo, redo, and
    `AddEffect`/`SetEffectParam`/`SetEffectKeyframes`/`ClearEffectKeyframes` for
    both nodes, including a 16-point curve on all four channels, preserve values
    and vector position exactly. Out-of-range, wrong-type, unknown-parameter,
    non-increasing-`x`, and illegal-keyframe cases are rejected atomically with
    `field` + `observed` + `allowed` asserted.
11. **Agent plan-not-apply.** `plan_color_wheels` and `plan_color_curves` return
    exact operations, bind to the analyzed revision, fail closed on a stale
    revision, and leave the source document byte-identical. Manifests list ordered
    stages with bypass flags and resolved curve points.
12. **Proof parity.** A clip with all three node kinds renders identically through
    preview and the full-raster monitor proof (media fixture, which also checks
    the CPU-reference monitor gate, provenance, and the serialized
    `clip.effects` order), and the `render_color_proof` manifest's ordered
    `color_nodes` stage names, bypass flags, and resolved curve points match
    `clip.effects` (agent test; media exposes no stage manifest of its own).

## 11. Explicit deferrals

- Hue-vs-hue, hue-vs-saturation, saturation-vs-saturation, and luma-vs-saturation
  curves (CC5).
- HSL qualifiers, geometric windows, feathering, tracking, node-scoped mattes, and
  matte inspection (CC5).
- LUT/look management, project-owned hashed assets, shapers, and adjustable look
  mix (CC4).
- Curve/wheel presets, grade copy-paste between clips, node groups, and
  still-store galleries (post-CC5 workflow).
- Log or camera-native grading encodings, CCT-in-Kelvin controls, chromatic
  adaptation, gamut mapping, HDR, and any transfer beyond the CC1 contract
  (CC6/CC7).
- Automatic curve or wheel derivation from scopes; CC3 planners remain
  evidence-only and request-driven.

CC3 is complete only when a colourist can build an ordered wheels-and-curves grade
by hand or from an agent proposal, bypass any node losslessly, undo any gesture in
one step, and independently verify the result against the equations in §2.

## 12. Implementation order

1. **Core descriptors and validation.** `crates/kinewright-core/src/effect.rs`
   (`EffectUniform::ColorNode`, `color_wheels`, `color_curves` descriptors, a
   `color_curve_parameter_names` helper, a `MANAGED_COLOR_NODE_NAMES` list);
   `crates/kinewright-core/src/operation.rs` (curve `x` ordering validation,
   `NonHoldKeyframeParameter`, `CurvePointCountAnimatedWithPoints`,
   `InvalidCurvePoints`, `too_many_color_nodes`); core unit tests.
2. **CPU reference math.** `crates/kinewright-media/src/color_pipeline.rs`
   (`grade709` encode/decode, `ColorWheels`, `ColorCurve` with an independent
   Fritsch–Carlson solve and Hermite evaluation, `ColorNode` enum,
   `apply_color_nodes` replacing `apply_primary_corrections` while keeping the
   old symbol as a thin wrapper).
3. **GPU ABI and shader.** `crates/kinewright-media/src/compositor.rs`
   (`grade_buffer_bytes` replacing `primary_buffer_bytes`, node-kind tag,
   bypass/neutral skip, host tangent solve, new binding-size constants);
   `crates/kinewright-media/src/compositor.wgsl` (`GradeBuffer`,
   `grade709_encode/decode`, `apply_wheels_node`, `curve_eval`,
   `apply_curves_node`, dispatch by kind in `apply_color_nodes`).
4. **Fixtures.** `crates/kinewright-media/src/cc3_fixtures.rs` (new), registered
   in `crates/kinewright-media/src/lib.rs`; reuses `cc1_fixtures.rs` helpers for
   provenance, diff metrics, and evidence emission.
5. **Agent surface.** `crates/kinewright-agent/src/color_status.rs`
   (`plan_color_wheels`, `plan_color_curves`, `color_nodes` manifest);
   `crates/kinewright-agent/src/server.rs` (dispatch);
   `crates/kinewright-agent/src/schema.rs` (tool names 64→66, compact
   `color_curves` descriptor summarization).
6. **Human UI.** `crates/kinewright-app/src/inspector_ui.rs`
   (`color_wheels_section`, `color_curves_section`, bypass toggle, per-node
   reset, hide raw point parameters from the generic slider loop); new
   `crates/kinewright-app/src/color_wheel_widget.rs` and
   `curve_editor_widget.rs`; gesture coalescing through the actor's coalesced
   batch command.
7. **Docs.** This file; `docs/ROADMAP-AND-WORKFLOWS.md` current-status and staged
   table rows; `CHANGELOG.md`.

Steps 1→2→3 are strictly ordered. Step 4 depends on 2 and 3. Steps 5 and 6 depend
on 1 and can proceed in parallel with 4.

## 13. Risks

- **Tool-schema bloat.** 133 `color_curves` parameters flow into the agent tool
  description string via `schema.rs`. Without the compact summarization in §2.4
  this measurably degrades agent runtime efficiency (M36). High likelihood, cheap
  fix, must not be skipped.
- **Storage-buffer portability.** The buffer grows from 64 bytes to up to ~13.6 KB
  and gains a payload region. Downlevel GL/WebGL backends and any device
  negotiated without `compositor_required_limits` will fail. Mitigation: raise the
  constant, assert it in the existing limit-contract test, and keep the binding
  count at 1.
- **Neutral short-circuit discontinuity.** One integer step away from neutral
  activates the `E`/`D` round trip. Because the pair is an exact bijection the
  step is at ULP scale, but the fixture must measure and record it rather than
  assume it.
- **f32 divergence in the tangent solve.** The Fritsch–Carlson pass has data-
  dependent branches; a compiler reassociating `a*a + b*b > 9.0` on one platform
  could flip a limiting decision at the boundary. Mitigation: the host solves
  tangents once (not in the shader), and a fixture includes a point set exactly on
  the `a² + b² = 9` boundary.
- **UI surface size.** Two custom widgets plus keyframe indicators is the largest
  app-side slice since M13; the widgets must be pure functions of the document so
  they stay testable without a window.
