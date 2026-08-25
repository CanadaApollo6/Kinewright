# CC1 managed SDR primary correction

Status: implementation contract, 2026-08-24
Depends on: [CC0 and the colour workflow in the roadmap](ROADMAP-AND-WORKFLOWS.md)
Scope: one technically correct, editable SDR correction path from a known source
to the current Rec.709 delivery target.

CC1 is a technical workflow, not an auto-grade or a new taste benchmark. It must
make a shot predictable before it makes the shot attractive. The editor should be
able to select one clip, understand the source and target colour descriptions,
adjust primary controls while watching scopes or a proof, and know that preview,
proof, and export are the same transform.

The words **must**, **must not**, and **may** in this document are normative.

## 1. In scope and out of scope

CC1 delivers:

- a managed SDR path for the supported source profiles below;
- a scene-linear Rec.709 working representation stored as `Rgba16Float`
  (`ColorBitDepth::Float16`) with f32 CPU/shader arithmetic and no intermediate
  display-range clamp;
- one serializable primary-correction effect with exposure, white balance,
  contrast/pivot, tonal balance, and saturation controls;
- deterministic input, correction, compositing, monitoring, and delivery ordering;
- one canonical math/ordering contract implemented by the production GPU
  compositor for preview, full-raster proof, and export, plus an independent
  CPU reference implementation used only for parity tests; and
- objective fixtures for transform math, identity, controls, parity, and failure
  behaviour.

CC1 does not deliver HDR, camera-RAW decoding, ACES/OCIO, calibrated-monitor
output, log-to-display transforms, creative LUT management, curves, wheels,
secondaries, tracked mattes, shot matching, gamut mapping, or automatic correction.
Those are later slices (principally CC2--CC6). An unsupported source must produce a
visible, typed status and an explicit override/recovery path; it must not be
silently treated as Rec.709.

## 2. Colour descriptions and managed state

CC0's four descriptions remain distinct. CC1 gives each one an exact role:

| Description | CC1 value | Purpose |
| --- | --- | --- |
| `source` | The asset's probed or user-overridden `ColorDescription` | Describes source-coded samples. It is not changed by rendering. |
| `working` | BT.709 primaries, `linear` transfer, `rgb` matrix, full range, D65, `float16` storage | The internal scene-linear RGB domain. Values are not clipped to 0..1; controls and shader/CPU arithmetic are f32 before each half-float storage boundary. |
| `monitoring` | BT.709 primaries, BT.709 display transfer, `rgb` matrix, full range, D65, f32 arithmetic before display quantization | The preview/proof monitor target. The final display buffer is RGBA8 only at the boundary. |
| `delivery` | BT.709 primaries, BT.709 transfer, BT.709 matrix, limited range, D65, 8-bit | The current H.264/YUV420P export contract. Tags must be written explicitly. |

`working`, `monitoring`, and `delivery` are project state and must round-trip in
the project JSON. Runtime decoder details, GPU choice, and cache state are not
project state. The working description is a contract for the intermediate, not a
claim that the source file is linear or float. `Rgba32Float` is not a CC1
requirement: CC1 uses `Rgba16Float` because its render-target and blend support
is available across the supported wgpu backends; no implementation may assume
that `Rgba32Float` is blendable everywhere.

### 2.1 Supported source profiles

The managed path accepts only these source descriptions. A profile match is based
on all listed fields; a partial match is not enough.

| Profile id | Primaries | Transfer | Matrix | Range | White point | Integer depth |
| --- | --- | --- | --- | --- | --- | --- |
| `rec709_video` | `bt709` | `bt709` or `bt1886` | `bt709` or `rgb` | `limited` or `full` | `d65` | 8..=16 bits |
| `srgb_full` | `srgb` or `bt709` | `srgb` | `rgb` or `identity` | `full` | `d65` | 8..=16 bits |

`ColorBitDepth::Integer(n)` is accepted only for `8 <= n <= 16`; the named
integer variants are equivalent. The decoded FFmpeg pixel format must be one
that the explicitly configured swscale path can convert to `RGBA64` without
discarding the declared integer depth before Kinewright's transfer math. In
particular, a 10-bit source must not be silently converted to an 8-bit decoder
surface. If swscale cannot preserve the declared integer depth for a source
format, CC1 reports `unsupported_decoder_format` rather than falling back to
RGBA8. Float input, unknown depth, and non-integer camera samples are deferred
until the decoder contract can preserve their source values. The source
confidence and provenance remain inspectable evidence; a high-confidence value
does not make an unsupported tuple supported.

BT.709 and sRGB use the same D65 display primaries for CC1, but their transfer
functions remain distinct. A BT.709 stream with an unknown white point may use the
normative D65 value only through an explicit `profile_assumption` recorded in the
colour status/proof. The raw source metadata remains `Unknown`; no code may simply
rewrite it to D65 during decode. An unknown primaries, transfer, matrix, range, or
depth is `needs_color_override` and blocks managed proof/export.

The following are explicit CC1 failures, not guesses:

- BT.2020, P3, BT.601/170M, or any other non-BT.709/sRGB primaries;
- PQ, HLG, log, LogC, Log3G10, or any transfer other than the table above;
- unknown or unsupported matrix/range/depth;
- camera RAW, Bayer, floating scene-linear, or untagged image data; and
- a user-selected source override that still does not match a supported profile.

The error must name the asset, the unsupported field, the observed value, and the
allowed values. Rendering a black frame or silently using the default context is
not a valid fallback.

### 2.2 Working and output invariants

The managed renderer must assert these invariants at its boundary:

1. Input samples have a supported source profile and a declared integer depth.
2. FFmpeg decode is converted through an explicitly configured swscale
   BT.709/range path to `RGBA64` (16-bit integer RGBA). The conversion preserves
   the declared source depth and performs only the source matrix/range operation
   where that swscale path supports it; planar RGB limited-range input receives
   its one explicit Kinewright range expansion immediately after the boundary
   because swscale does not expand that RGB range. It must not apply a display
   transfer or a Kinewright primary control.
3. Kinewright converts the `RGBA64` coded RGB values to f32 linear BT.709 and
   stores the working surface as `Rgba16Float`/`ColorBitDepth::Float16`. Negative
   values and values above 1.0 may exist between stages; all arithmetic remains
   f32 even though the storage surface is half-float.
4. Alpha and geometry are independent of RGB colour correction. A layer mask is
   not a CC1 secondary and must not be used as one.
5. No colour stage clamps RGB to 0..1. The only RGB clamp is in the final monitor
   or delivery encoding step.
6. Monitor and delivery output transforms are selected from the corresponding
   `ColorDescription`, never from an FFmpeg or GPU default.

## 3. Deterministic pipeline

For every video layer, the canonical order is:

```text
source coded samples
  -> source range expansion
  -> source matrix decode to coded RGB
  -> source transfer decode to linear light
  -> primaries/white-point conversion to working BT.709 D65
  -> primary correction nodes, in serialized clip.effects order
  -> non-colour layer operations and linear-light layer compositing
  -> monitoring transform (preview/proof) OR delivery transform (export)
  -> final clamp, quantization, and display/codec packing
```

For CC1, source and working primaries are the same for the accepted profiles, so
the primaries conversion is an identity matrix. It is still a named stage so that
CC2+ can add real conversion without changing the order or silently turning a
display transform into a grade. A BT.709 white-point assumption is likewise a
named, inspectable step.

The current `Effect` vector remains the serialization order. A canonical
`primary_correction` effect is the CC1 node. Multiple nodes are legal and execute
in vector order, although the human workflow creates one node per clip. The
renderer must not flatten separate nodes into one opaque set of uniforms. Existing
geometry, opacity, crop, and transition operations remain separate from the colour
node. Creative looks and LUTs, if present in a legacy project, are reported as
post-primary legacy stages and are outside the CC1 conformance claim; CC4 will give
them a first-class ordered node contract.

### 3.1 Source decode, range, and transfer math

The bounded CC1 decoder path is deliberately not a requirement to expose native
Y, Cb, and Cr planes to the rest of Kinewright. FFmpeg decodes the source frame,
then an explicitly configured swscale conversion uses the declared BT.709 matrix
and declared full/limited range to produce `RGBA64` (RGBA16-bit integer). The
swscale context must be configured from the source `ColorDescription`; no
unspecified/default matrix, range, transfer, or colour-space selection is
allowed. For `rec709_video`, that means the explicit BT.709 matrix and the
declared full/limited range; for `srgb_full`, it means the explicit RGB/identity
matrix and full range. For a `rec709_video` source whose matrix is `rgb`, the
swscale graph still produces coded planar RGB but does not apply the requested
limited-range expansion; Kinewright performs that one native expansion after
normalization. Swscale is a bounded sample-format plus matrix/range boundary,
not the colour-management authority.

After that boundary, Kinewright owns normalization, transfer decoding, every
primary-control equation, output transfer, output range, and final packing. The
`RGBA64` bytes are integer-promoted swscale output, not necessarily a native
16-bit code with `65535` as its nominal white. For an `N`-bit full-range YUV
source, FFmpeg's direct swscale promotion is `C_rgba64 = C_native << (16-N)`;
therefore Kinewright uses:

```text
P_N = (2^N - 1) << (16-N)
E = C_rgba64 / P_N
```

For example, `P_8 = 65280`, `P_10 = 65472`, and `P_16 = 65535`. A correctly
tagged full-range BT.709 8-bit ramp therefore maps source codes 18 and 255 to
`RGBA64` codes 4608 and 65280, respectively; dividing those bytes by 65535
would make white `0.9961` and is not the CC1 normalization.

**Erratum (2026-08-25, found by the CC6 probe).** The same `P_8 = 65280`
convention governs swscale's *input* side for 16-bit RGB: when the export
filter graph is fed `rgba64le`, libswscale treats `255 << 8 = 65280` as nominal
white, not `65535`. The delivery quantizer had scaled the clamped encoded value
by `65535`, so nominal white encoded to Y′ 236 (8-bit) and mid-grey ran about
0.6 code high; the tolerance window of the filter unit test hid it. The
delivery intermediate is now defined as `DELIVERY_INTERMEDIATE_WHITE = 65280`
(`C_rgba64 = round(E' · 65280)`), white encodes to Y′ 235 exactly, and the
decoded-delivery reference in the CC1 fixture is `round(255 · C / 65280)`.
The intermediate exists only to feed the export graph; nothing else consumes
it on the `65535` scale. Alpha in the intermediate is quantized on the same
scale so the intermediate has exactly one scale to invert; the export graph
discards it at `format=yuv420p`.

The direct swscale range/matrix path has two additional, explicit effective
scales. Limited BT.709 YUV-to-RGB conversion uses FFmpeg's 8-bit fixed-point
RGB scale even when the source planes are 10 bits (or deeper), so its nominal
legal-white denominator is `P_8 = 65280`, not `P_N`. With the pinned FFmpeg
build, legal black is `0`, legal white is `65283` after fixed-point rounding,
and a 10-bit legal-range midpoint is `33387`; these are expected boundary
rounding observations, not reasons to apply a second range expansion or to
calibrate a denominator from the frame's observed maximum.

Planar RGB is a separate swscale case. Even with the declared
`in_range=mpeg` option, swscale does not expand the RGB channels in this direct
graph: 8-bit codes are promoted on the `P_8` scale, while RGB input deeper than
8 bits uses a true 16-bit scale (`65535`). For an accepted Rec.709
RGB/limited source, Kinewright therefore normalizes with that effective scale
and performs the one required native
limited-range expansion per channel before transfer decoding. This is not a
second expansion: swscale did not expand the planar RGB range. Full-range
planar RGB skips that expansion. The typed source depth is still validated
independently; these effective denominators describe only the explicitly
configured FFmpeg boundary and never suppress range or matrix errors.

The source pixel format is supported only when the selected swscale conversion
can preserve the declared integer depth while producing this configured RGBA64
result. A failed preservation check is an explicit `unsupported_decoder_format`
error, never a best-effort RGBA8 path. RGB channels may retain over-range values
after division; no normalization step clamps them. Alpha is different: it is
always normalized as `A = A_rgba64 / 65535.0`, because alpha is an actual
16-bit destination channel rather than a promoted source colour code.

All Kinewright colour arithmetic below is f32 unless a CPU reference explicitly
uses higher precision and rounds to f32 at the named stage. The transfer formulas
below consume post-swscale `E = C_rgba64 / D`, where `D` is the selected effective
RGB denominator described above (`P_N` for full-range YUV, `P_8` for direct
limited BT.709 YUV or 8-bit planar RGB, or `65535` for planar RGB deeper than 8
bits). Alpha alone uses `A_rgba64 / 65535`. The native source-code equations that follow are
fixture-reference evidence for the configured matrix/range conversion, not the
direct normalization algorithm after swscale. For those reference equations, let
`N` be the integer bit depth, `S = 2^(N-8)`, and `M = 2^N - 1`.

For fixture reference only, full-range native RGB code `C` becomes `E = C / M`.
For limited-range native video, luma uses `E = (C - 16*S) / (219*S)` and chroma uses
`E = (C - 128*S) / (224*S)`. The range expansion is allowed to produce values
outside 0..1 for overshoot; it is not clamped before matrix conversion. BT.709
limited Y'CbCr uses:

```text
R' = Y' + 1.5748 * Cr
G' = Y' - 0.187324 * Cb - 0.468124 * Cr
B' = Y' + 1.8556 * Cb
```

The coefficients are part of the CC1 contract and must not come from a backend's
undocumented default. For RGB/identity input, range expansion is applied per
channel and the matrix step is skipped. The fixture compares swscale's explicit
RGBA64 result with these native-code expectations before applying Kinewright's
transfer function; it does not divide the resulting RGBA64 channels by `M`.

BT.709 transfer decoding is:

```text
linear = E / 4.5                                  if E < 0.081
linear = ((E + 0.099) / 1.099)^(1 / 0.45)          otherwise
```

sRGB decoding is:

```text
linear = E / 12.92                                if E <= 0.04045
linear = ((E + 0.055) / 1.055)^2.4                 otherwise
```

For the CC1 SDR contract, BT.1886 is the zero-black-level power transfer
`linear = max(E, 0)^2.4`. BT.709 monitoring uses the following forward function;
the negative extension is sign-preserving so recoverable undershoot survives until
the final output clamp:

```text
E = 4.5 * linear                                  if abs(linear) < 0.018
E = sign(linear) * (1.099 * abs(linear)^0.45 - 0.099) otherwise
```

The strict comparison is intentional: exactly `0.018` takes the nonlinear branch,
just as exactly `0.081` takes the nonlinear inverse branch. The rounded BT.709
constants leave a small real-valued discontinuity at that seam; the fixture records
it explicitly and separately proves that every supported 8-bit and 10-bit integer
neutral ramp remains monotone after the complete transform. Transfer thresholds and
constants are not delegated to a platform colour API.

### 3.2 Primary correction controls

The canonical effect name is `primary_correction`. All parameters are integer
`ParamValue::Integer` values and are valid for automation/keyframes under the
ordinary Core operation rules. Defaults are inserted by the human/agent workflow;
an omitted parameter resolves to its neutral value. Bounds are inclusive.

| Parameter | Stored unit | Minimum | Maximum | Neutral | Meaning |
| --- | --- | ---: | ---: | ---: | --- |
| `exposure_milli_stops` | 1/1000 stop | -5000 | 5000 | 0 | Multiply linear RGB by `2^(value/1000)`. |
| `temperature_percent` | signed percent of the CC1 warm/cool control | -100 | 100 | 0 | Positive is warmer; zero is an identity. |
| `tint_percent` | signed percent of the CC1 green/magenta control | -100 | 100 | 0 | Positive is magenta; zero is an identity. |
| `contrast_percent` | signed percent | -100 | 100 | 0 | Contrast scale `1 + value/100` around pivot. |
| `contrast_pivot_basis_points` | 1/10000 of working display white | 0 | 10000 | 5000 | Pivot used by contrast; 0.5 is neutral default. |
| `blacks_percent` | signed percent of a 0.25 linear-unit lift | -100 | 100 | 0 | Low-end endpoint adjustment. |
| `shadows_percent` | signed percent of a 0.20 linear-unit lift | -100 | 100 | 0 | Lower-midtone adjustment. |
| `highlights_percent` | signed percent of a 0.20 linear-unit lift | -100 | 100 | 0 | Upper-midtone adjustment. |
| `whites_percent` | signed percent of a 0.25 linear-unit lift | -100 | 100 | 0 | High-end endpoint adjustment. |
| `saturation_percent` | signed percent | -100 | 100 | 0 | Mix luma with RGB; -100 is monochrome and 0 is identity. |

The percent controls use integer percentage points, not a UI-dependent float. A
value at a bound is valid and must not be silently widened or clamped by the
renderer. The descriptor table is the validation authority; the compositor may
clamp only derived values needed to form a finite final output.

The canonical control order is:

1. white balance;
2. exposure;
3. blacks, shadows, highlights, and whites;
4. contrast around `contrast_pivot_basis_points`; and
5. saturation around Rec.709 luma.

The CC1 reference implementation defines white balance as a diagonal linear-RGB
gain: `temperature_percent / 100` changes red and blue by equal and opposite
10% gains (positive is red up/blue down), while `tint_percent / 100` changes green
by the opposite 10% gain (positive is green down). Gains are applied around unity
and must remain non-negative at the bounds. This intentionally bounded primary
control is not a CCT estimator; temperature in Kelvin, chromatic adaptation, and
shot matching are deferred.

For tonal balance, let `x` be the current linear channel and `u = clamp(x, 0, 1)`
be used only to calculate weights. Define `smoothstep(a,b,u)` in the usual
Hermite form and:

```text
w_black    = 1 - smoothstep(0.00, 0.25, u)
w_shadow   = 1 - smoothstep(0.15, 0.50, u)
w_highlight = smoothstep(0.50, 0.85, u)
w_white    = smoothstep(0.75, 1.00, u)

x += 0.25 * blacks/100       * w_black
x += 0.20 * shadows/100      * w_shadow
x += 0.20 * highlights/100  * w_highlight
x += 0.25 * whites/100       * w_white
```

The weights use clamped `u`, but `x` itself is not clamped. Contrast then uses
`x = pivot + (x - pivot) * (1 + contrast/100)`, and saturation uses
`luma = 0.2126*R + 0.7152*G + 0.0722*B` followed by
`RGB = luma + (RGB - luma) * (1 + saturation/100)`. These equations are the
cross-backend reference; an implementation may vectorize them but must preserve
their results within the tolerances in Section 6.

## 4. Core representation and migration

CC1 should use the existing typed Core edit path:

- `AddEffect` creates the `primary_correction` node;
- `SetEffectParam` changes one control; and
- `SetEffectKeyframes`/`ClearEffectKeyframes` manage clip-local automation.

Each change must validate against the descriptor table, be revision-gated at the
agent boundary, journal as an ordinary operation, and support undo/redo. A
parameter outside the table's inclusive range is rejected atomically. The Core
model must not store floats or UI labels for these controls.

The project needs an explicit managed-colour state/version for migration. If the
implementation adds a field, its absent value is `legacy`; the first CC1 save
writes `managed_sdr_v1`. The migration rules are:

1. A pre-CC0 project with no `color_context` receives the CC0 explicit unknown
   source descriptions and the current SDR Rec.709 monitor/delivery defaults. It
   is not automatically declared source Rec.709.
2. A CC0 project whose working description is the old application-default
   BT.709/8-bit/full placeholder is migrated to the fixed linear-`Rgba16Float`
   (`ColorBitDepth::Float16`) working description. That field was not an executed
   transform; the migration must not reinterpret source pixels as already linear.
3. Existing `color_grade` parameters copy to a new `primary_correction` node:
   `exposure_milli_stops`, `temperature_percent`, and `tint_percent` are copied
   exactly; new parameters resolve neutral. The old node id and effect position
   are retained when possible so journal references remain understandable.
4. Existing standalone `brightness`, `contrast`, and `saturation` effects are not
   silently translated to linear-light primary controls because their old shader
   semantics were display-coded and are not mathematically equivalent. They are
   retained as legacy display-coded compatibility entries, rendered only by the
   compatibility path, and cause CC1 conformance, colour status, and proof
   manifests alike to report `legacy_colour_semantics`
   until the editor accepts a visible conversion proof. No silent visual change is
   allowed.
5. Existing built-in looks and external `.cube` LUTs remain compatibility stages
   after primary correction. Their ordering and file portability are not CC1 exit
   evidence; missing LUTs are an explicit error. CC4 owns their managed migration.
6. Save/reopen, journal replay, undo, and redo must preserve the migrated node,
   source metadata, and managed state byte-for-byte apart from documented JSON
   defaults.

If a project contains a custom colour context that cannot be represented by the
CC1 working/monitoring/delivery contract, load remains possible but managed proof
and export are blocked with the exact incompatible field. The editor must be able
to reset the target explicitly; loading must not rewrite it silently.

## 5. Preview, proof, export, and human/agent observability

Preview, proof, and export all use the production GPU `FrameRenderer`/compositor
and the same managed transform semantics, receiving the same source frame. The
CPU implementation is an independent reference used by the parity fixtures; it
is not a production proof renderer. The only permitted production differences
are the selected final target (`monitoring` versus `delivery`), final raster
size, and codec quantization. A thumbnail, proxy, or stale cache cannot establish
CC1 conformance.

Every colour status/proof reports:

- source profile and the raw `ColorDescription` provenance/confidence;
- any explicit profile assumption, especially inferred D65;
- working, monitoring, and delivery descriptions;
- the ordered stage names and primary parameter values;
- active visual layers in production z-order, including source-backed layers'
  ordered evaluated effect parameters and fully resolved primary parameters;
- whether the frame is production GPU preview/proof, independent CPU reference
  evidence, or decoded delivery;
- input/output bit depth, range, raster, and sampling region; and
- unsupported metadata or legacy-stage warnings.

Human controls and agent plans use the same integer parameter names and bounds.
The agent may propose a revision-gated `AddEffect`/`SetEffectParam` sequence with
before/after proof evidence, but CC1 analysis is evidence-only and never silently
mutates a grade. There is no CC1 auto-white-balance or auto-exposure operation.

## 6. Objective exit fixtures and tolerances

The CC1 gate is a fixture suite, not a subjective hero-shot review. Each fixture
records the git revision, renderer backend, OS, source profile, source depth,
working/monitor/delivery descriptions, control values, and output hashes.

### 6.1 Required fixtures

1. **Identity ramp:** full-range and limited-range Rec.709 ramps at 8 and 10 bits,
   plus an sRGB full-range ramp. The ramp must be monotonic after every neutral
   transform, black/white range expansion must be correct, and neutral gray must
   remain neutral.
2. **Neutral chart:** black, near-black, 18% gray, mid-gray, near-white, white,
   red, green, blue, cyan, magenta, and yellow patches. The chart exercises
   matrix, transfer, white balance neutrality, saturation, and output range.
3. **Primary controls:** one fixture per control at neutral, minimum, maximum,
   and a representative interior value. Exposure includes ±1 stop; contrast
   includes pivot preservation; tonal controls include low/high patches; and
   saturation includes -100 and 0.
4. **No-intermediate-clamp:** an over-range ramp is corrected with a negative
   exposure and must recover values that would be lost if the source were clamped
   after decode or between controls.
5. **Unsupported metadata:** unknown/partial metadata, BT.2020/PQ, HLG, log,
   unsupported depth, and unsupported matrix fixtures must block managed proof and
   name the recovery action.
6. **Migration:** pre-CC0 JSON, CC0 JSON, old `color_grade`, legacy display
   effects, save/reopen, journal replay, undo, and redo.
7. **Parity:** the same frame/control set through the independent CPU reference,
   software GPU (lavapipe/WARP where available), one supported hardware GPU in
   an explicit/manual lane when available, production monitor proof, and decoded
   H.264/YUV420P delivery. The hardware test is ignored by default so ordinary
   CI does not claim hardware coverage without a physical adapter. When no
   software fallback adapter exists, the default-lane GPU fixtures fail with an
   explicit message rather than skipping; setting
   `KINEWRIGHT_CC1_ALLOW_HARDWARE_GPU=1` lets them run on the hardware adapter
   with the evidence lane recorded as `hardware_optin` and honest
   `software_fallback=false`/`gpu_claim=true` provenance. The parity raster must
   contain samples that exercise every control, including highlights, whites,
   over-range, and negative values; a non-neutral control case that changes
   fewer than 5% of the CPU-reference samples fails as vacuous.

### 6.2 Numeric gate

The normative equations and stage ordering are the comparison contract. The
independent CPU reference implements that contract separately and is the
comparison source; production monitor proof is rendered by the GPU compositor.
For uncompressed RGBA8 monitor/proof frames:

- neutral identity: maximum absolute channel difference `<= 1` code value,
  P99 `<= 1`, and mean absolute difference `<= 0.25`;
- CPU versus GPU for any CC1 control: maximum `<= 2`, P99 `<= 1`, mean `<= 0.50`;
- channel neutrality on neutral chart patches: maximum channel spread `<= 1`;
- monotonic ramps: zero descending adjacent pairs after final encoding; and
- range endpoints: limited black/white and full black/white map to the expected
  monitor codes within `<= 1` code value.

For linear working values before final encoding, the CPU/GPU comparison includes
the normative `Rgba16Float` storage quantization and uses f32 arithmetic on both
paths. The gate is banded by the magnitude of the CPU-reference value, because a
half-float ULP doubles at 1.0: on finite samples with absolute linear values
`<= 1`, maximum absolute error `<= 1.5e-3`, P99 `<= 7.5e-4`, and mean absolute
error `<= 2.5e-4`; on samples with absolute values in `(1, 2]`, the maximum stays
`<= 1.5e-3` while P99 and mean are `<= 9.765625e-4` (exactly one half-float ULP
in that band). Samples above `2` are excluded from the linear gate, counted, and
remain subject to the monitor-code, finiteness, and monotonicity gates. Both
bands are asserted and recorded in the manifest. This is intentionally a
half-float bound (one to two ULPs), not a promise that an unproven RGBA32F blend
path is available; the 2026-08-24 review measured that a hardware Vulkan adapter
rounds the f32-to-f16 store toward zero while the CPU reference rounds to
nearest, so roughly 40% of samples differ by exactly one ULP and the over-range
band cannot meet the sub-ULP P99 that the in-gamut band meets.
The fixture also records first, median, and 99th-percentile luma, clipping in
basis points, and max/P99/mean absolute channel differences as required by the
roadmap.

The gate compares the GPU output pixel at `(x, y)` against the CPU reference
evaluated on the source texel at `(x, y)`. That correspondence is a renderer
obligation, not an assumption: when a layer's source raster has the output
raster's shape and no geometric stage (scale, offset, or reframe) moves it, the
compositor samples it with point filtering. Bilinear filtering is the identity
for such a layer only in exact arithmetic; Vulkan requires just a few bits of
sub-texel precision, and an implementation that reconstructs the sub-texel
coordinate in f32 may return a filter weight one ULP of the texel coordinate
away from zero. Mesa lavapipe does so (measured 2026-08-24: `2^-15` to `2^-14`
of the neighbouring texel blended into 32 of 3072 pixels of the CC3 §10.2
raster), where the NVIDIA adapter returns every texel exactly. A pixel-exact
layer must therefore sample bit-exactly on every adapter, and the parity
numbers above are stated on that basis. Layers that are genuinely resampled
keep bilinear filtering and are outside this clause.

For decoded H.264/YUV420P delivery, codec loss is measured separately from the
managed-render comparison: maximum channel difference `<= 4`, P99 `<= 2`, and
mean absolute difference `<= 1.0` against the delivery proof on the same raster.
The encoded stream must carry the explicit BT.709/limited tags accepted by the
current delivery conformance check. These codec tolerances must not be reused for
the compositor or CPU/GPU gate.

No tolerance may be used to excuse an unsupported source, a missing tag, an
intermediate clamp, a wrong transform order, or a stale/legacy colour stage.

## 7. Explicit deferrals and follow-on boundaries

The following are intentionally not hidden inside CC1:

- CCT-in-Kelvin temperature controls, Bradford/chromatic adaptation, and camera
  matrices (CC2/CC7);
- curves, lift/gamma/gain wheels, HSL qualifiers, windows, tracking, and matte
  scoped corrections (CC3/CC5);
- technical-versus-creative LUT nodes, project-owned hashed LUT assets, shapers,
  adjustable look mix, and look portability (CC4);
- gamut mapping, legal-range policy, skin diagnostics, HDR, and delivery profiles
  beyond Rec.709 8-bit H.264/YUV420P (CC6); and
- automatic shot matching or an unexplained `auto_grade` operation (CC2/CC7).

CC1 is complete only when a user can make a deterministic managed SDR primary
correction and independently verify it. More controls or a nicer hero shot do not
close this gate if the transform, migration, parity, or failure fixtures are not
passing.
