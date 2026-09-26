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
  adjustment Push. Ledger two pooled snapshots, not three. (ME10 makes a
  `Normal` adjustment Push 2 as well.)
- ME3 → §4 R16 (Part B1): the MO1 R26 re-gate runs each golden's raster,
  size and transform through `WorkingFrame::from_display_frame`, the input
  type every render composites, not the goldens' 8-bit `FrameTexture`.
  Evidence: lavapipe filters `Rgba8Unorm` at 8-bit precision (up to 0.008
  linear off an exact bilinear on the high-frequency gradient), which no
  twin reproduces portably. That path is fixture-only (production never
  composites 8-bit textures). With f16 inputs, GPU ≡ twin within 5e-4 on
  all 13 cases.
- ME4 → §7 R24 (Part B2): the argument is `clip_id`, not `clip` — every
  clip-targeted capability (`plan_motion`, the colour planners) spells it
  that way, and a lone `clip` would be the registry's only exception.
  `SoloError` carries two variants beyond the three R24 names:
  `InvalidSamples` (`samples` outside 2..16, which the schema cannot bound)
  and `RenderFailed` (a typed outer variant kept apart from
  `SoloClipNotVisible`; the proof path's inner `MediaError` is carried
  stringified, not as a typed payload). Refusals are budgeted like strips:
  one whose JSON body passes 4 KiB or whose serialized response passes
  1,056 KiB (a path-bearing render failure) is answered as a fixed-size
  `solo_over_budget` instead, and a working raster past the device's 8192-px
  texture side is refused `solo_over_budget` (`render_side`) in every mode
  before any product or allocation (fix round 1). Malformed arguments get
  one fixed-size JSON-RPC InvalidParams (`preview_solo: invalid arguments`,
  data code `solo_invalid_arguments`) instead of the decoder's message,
  which echoes the input; other tools keep their decoder messages (fix
  round 2). The budget is the whole reply on the wire (final fix): every
  `preview_solo` reply — strip, solo refusal, stale-revision text,
  InvalidParams or any other error, a panicked handler included (answered
  with the fixed text `tool call failed: handler panicked`, never its
  payload, as for every tool; N24) — crosses one choke point in
  `call_tool`, which measures it as rmcp serializes the JSON-RPC message
  (echoed request id and `resultType` included) plus
  `SOLO_FRAMING_BYTES` = 128, a bound on rmcp 3.1.2's SSE framing (priming
  event 25 B + reply event 13 B + two event ids of at most 41 B = 120 B).
  Outside the measure by definition: HTTP headers, HTTP chunk framing and
  SSE keep-alive comments (sent only while a render passes 15 s; transport
  liveness, not reply bytes). A reply measuring over
  1,081,344 B is replaced by the minimal typed refusal —
  `{"resultType":"complete","content":[{"type":"text","text":"solo_over_budget"}],"structuredContent":{"code":"solo_over_budget"},"isError":true}`
  (142 B), a 175-B message around the id, 303 B with framing — whenever
  that is smaller (an InvalidParams already is, and stays). **Residual:** a
  dispatched reply — one the transport hands to `call_tool` — can pass the
  budget only when the request id's JSON is longer than
  1,081,344 − 303 = 1,081,041 B, since the protocol must echo the id.
  rmcp's pre-dispatch refusals are outside it: they predate MO2, cover
  every tool and never reach the choke point (−32020 quotes the
  request's `_meta` protocolVersion; a malformed id gets a silent HTTP
  202). They belong to AW2 under AW2-OBL-1, which requires every
  transport refusal to be bounded and echo-free and a malformed id to get
  −32600 (N21, N24). A `context: isolated` sent for an
  adjustment is answered as `below` — the report says so — rather than
  refused, since R24 says "no override". Codes are `solo_clip_not_visible`,
  `solo_window_empty`, `solo_over_budget`, `solo_invalid_samples`,
  `solo_render_failed`.
- ME5 → §12 staging (N13): Part B2 split under §12's > 20% rule. B2 is now the
  solo transport, the registry re-pin and the R28 floors/ledger; a new
  **B3** carries the R26 person GUI and the §8 parity checklist (budget
  ≤ 700, stop at > 840). Evidence: B2 stopped at 423 landed lines plus a
  634-line GUI, projecting 1,140–1,210 against ≤ 800. Thinning (moving the
  viewer drag and wedge glyphs to MO6) was rejected as a change to R26. B3
  landed at 688 production lines and stands at 830 after fix round 1 (inside
  the 840 stop), over S7's ≤ 400 for GUI gestures + menus, because it also
  holds the solo strip dialog, its context/sample controls and the parity
  seams. The final ledger (N21) is Part A 691, B1 ≈ 1,713, B2 ≈ 715
  (solo/registry 524, R28 191) and B3 830: ≈ 3,950 against 3,200, about
  23% over and past §12's 20%. The overage is accepted through the
  recorded overrides: B1's review-fix growth (N15, N18), R28's
  resident-source scope (N19, N20) and its 191 lines (N21). R28 ran after
  the B1 review fixes, since those change the render hot path.
  **R28 now (N23).** The 191 was the pre-fix snapshot. The final
  verification fix added 13 production lines and removed 1, giving
  204 added / 29 removed; ME14 added none. ME15 adds 107 and removes 45
  (`git diff -U0`). Fourteen of those removals are R28's own earlier
  lines (11 from the final-verification fix, 3 from the first R28
  commit). The other 31 are earlier code: 20 pre-MO2 lines (the
  `write_texture` call) and 11 MO2 B1 lines (imports, the stage-layer
  call site). R28 therefore stands at **297 added / 60 removed** (net
  237), counted like the re-review's `current-line-count.json`:
  `compositor.rs` production code, blank lines included, `#[cfg(test)]`
  items excluded. The solo/registry 524 also moves: the final fix
  (af96c8e) added 71 production lines and removed 59 (net +12), and the
  N24 fix adds 9 and removes 1 (net +8), giving 544. B2 is then
  ≈ 544 + 297 = 841 and the total ≈ 691 + 1,713 + 841 + 830 = 4,075,
  about 27% over 3,200. The ME15 growth was lead-ordered (N23) and the
  choke-point fixes review-ordered (N21, N24); those overrides carry
  them.
  **R28 after ME16 (N25).** ME16 adds 186 production lines and removes
  86 (`git diff -U0`, the same count). Six of the added lines are
  `#[cfg(test)]` hook calls inside production functions, counted anyway.
  Twenty-eight of the removals are ME15's own lines (the upload helper
  and the cleanup's inline submit, now shared). The other 58 are earlier
  code: 42 pre-MO2 lines (the atlas `write_texture`, the readback wait)
  and 16 MO2 B1 lines (the readback callers). R28 therefore stands at
  **455 added / 118 removed** (net 337). B2 is ≈ 544 + 455 = 999 and the
  total ≈ 691 + 1,713 + 999 + 830 = 4,233, about 32% over 3,200. The
  ME16 growth was lead-ordered (N25), and that override carries it.
- ME6 → §8 R26 (Part B3), readings the rule leaves open:
  - Solids:
    - The colour editor is egui's picker plus labelled R/G/B fields. A
      picker session, a channel drag or one typed-entry session (from the
      field taking focus) is one undo step.
    - A new solid is mid-grey `#808080`.
  - Adjustments:
    - A new adjustment carries an empty look.
    - Its "intended affected tracks" are the selected clip's track.
      Without a selection, they are every video track with content in the
      span.
  - A placement refusal is a formatted typed refusal: the pure
    `PlacementRefusal` (span, kind, the track to sit above, or a default
    span past the last frame) is rendered as text into the Operations
    incident log; the struct itself is not retained there.
  - The viewer's transform overlay:
    - It yields to an expanded matte section (CC5 owns that pointer).
    - It writes through MO1's auto-key rule: keyed params get a key at the
      playhead, others a static.
    - Its outline, corner handles and scale pivot are the rendered layer:
      the enabled effects' master, per-axis and fine scale, coarse and fine
      offsets, rotation and anchor, as the compositor resolves them.
  - The solo dialog never downscales: a strip past the GPU's texture side
    (a default 8-sample strip is 2,560 px; egui's default side is 2,048) is
    held as native-size tiles. Full-res opens at 1:1 (scroll to pan); a
    fitted view says "shown at N%". The dialog also sends R24's context
    (by blend, isolated, over below-stack) and sample count (2–16) — GUI
    parity with the agent's arguments, not an exception (fix round 1).
- ME7 → §3 R10 (B1 fix round 1, review-1 B1 / review-2 B2 + S1): R10's
  checked set is every *special* layer — non-`Normal` blends **and every
  adjustment, `Normal` included** (selector word 8: a `Normal` adjustment
  validated against its snapshot; under Push, against the re-snapshotted
  shifted backdrop, per ME10). The source, the
  below value, the blend result and alpha must be finite, tested on the
  bits before `min`/`max` can erase them; magnitude is checked only on the
  value the target stores (`α·B + (1−α)·D`), so `Add(40000,40000)` at
  α=0.25 stores 49,984 instead of refusing. **Scope (lead ruling N15.3,
  reversed by N17.1 — see ME11):**
  `Normal` *pixel* layers stay unchecked in MO2. Three things rule it: the
  CC3 overflow contract (`cc3_boundary_controls_…overflows_to_infinity`),
  R12's untouched all-`Normal` fast path, and CC8 R35. Review-2's
  `(Normal, +inf)` pair-lanes case was therefore ruled out, not dropped
  silently.
- ME8 → §4 R14 / §6 R21 (B1 fix round 1, review-2 B1 + S2): no NDC
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
- ME9 → §9 R27 (B1 fix round 1, Windows CI run 36222189672, WARP): a
  GPU≡twin working value may also differ by a **derived sub-texel
  envelope**. D3D requires only 8 bits of sub-texel filter precision
  (Vulkan's `subTexelPrecisionBits`; lavapipe and NVIDIA report 8). The
  bilinear value is multilinear in its weights, so every rounding of
  `(fx, fy)` to 2⁻⁸ lies between the four floor/ceil corners. The twin
  renders those four corners, and the per-value slack is the largest
  `|corner − exact|` (≤ 2⁻⁸ × the local neighbour contrast per axis),
  carried through the rest of the pipeline. The slack is zero for
  unfiltered values, so they keep the unit 1e-3 (pinned on a blit); a
  filtered value's slack is not necessarily nonzero (corners can cancel).
  *(ME11 restricts the envelope to a proved subset.)* It is computed only when a value
  misses the exact R27. Evidence (emulated, CPU): WARP's R26 gradient
  value 290 departs by 0.00146 against a slack of 0.00244. The
  `slide_right` title at frame 3, pixel 6708, departs by 0.00122 against
  a slack of 0.00171; the twin reproduces CI's exact value there. Neither
  failure was a coverage tie. Adapters with fewer than 8 bits are out of
  scope.
- ME10 → §3 R9b (B1 fix round 2, lead ruling N15.1): a special layer
  (selectors 1–6 and 8) **composites the over in its shader and emits
  `(α·B + (1−α)·D, 1)`**, where D is the snapshot it already samples. It
  no longer emits `(B, αs)`. The fixed-function state is unchanged, so it
  stores the emitted value: `1·out + 0·D`, alpha `1·1 + 0·1`. `Normal`
  pixel layers (selector 0) and the R12 fast path are untouched; their
  pre/post identity and CC8 G2 SDR identity are re-run on both lanes.
  - *Evidence.* ME7 checks magnitude only on the stored value. On the
    RTX 3090, `Add(40000,40000)` at α=0.25 over D=40000 stored **46,368**,
    not 49,984. The fixed-function unit clamped the source B=80,000 to
    65,504 before blending: `0.25·65504 + 0.75·40000 = 46,376 → 46,368`.
    That silently stored a wrong value (lavapipe stored 49,984).
  - *Why this is portable.* Blending an out-of-range f16 source is
    implementation-defined. The shader already holds `B`, `D` and `α` in
    f32, so the only value crossing the blend unit is the representable
    result R10 has already checked.
  - *Consequence for Push.* The opaque emission overwrites every
    rasterized pixel, including pixels whose coverage α is 0. So `D` must
    be the true target. Every special entering layer under Push
    re-snapshots the shifted backdrop. A `Normal` adjustment Push
    therefore costs 2 copies, not 1: its quad can rasterize pixels
    coverage rejects (pinned by a moved adjustment in gate 5, which went
    red on the emission alone). The non-`Normal` Push re-snapshot is kept.
    R21 does not make it provably redundant, because the backdrop's
    in-raster test runs on the f32 fraction `x/n − q` while coverage uses
    the host's pixel edge, and near-ties can disagree. It is noted for
    R28 perf.
  - *Residual (superseded by ME12).* Both lanes' f32→f16 target store can
    round a value just above an f16 midpoint down by one ulp, where the
    twin rounds to nearest. This is within R27's 1e-3, but a uniform frame turns it into
    one monitor code on every pixel. The review's 60% legacy-cube Screen
    probe fails the mean gate on both lanes for that reason, so it runs
    at 70%.
- ME11 → §3 R10 (B1 fix round 3, re-review B1/B2/S1, lead ruling N17):
  - *R10 is universal.* Every layer flags on write when it would store a
    non-finite or over-f16 value, `Normal` pixel layers included; ME7's
    N15.3 scope is withdrawn. A `Normal` layer's store is the
    fixed-function over of its `(S, α)` onto a representable `D`, so it
    flags exactly when `S` or `α` is non-finite, or `α > 0` and some
    `|S| > 65504` — the cases whose over is non-finite or
    implementation-defined (the RTX 3090 clamps). The flag write is
    conditional: finite bytes, copies and the R12 fast path are unchanged
    (no schedule, no snapshot). An all-`Normal` frame binds a per-layer
    flag buffer recycled across frames (cleared in the frame's encoder,
    read back after the pixels), so it adds no per-frame allocation.
    Attribution stays sticky and names the clip and frame (pinned by
    `rereview_me7_valid_normal_solid_overflow`, covered and uncovered, on
    the working, twin, monitor and delivery paths).
  - *Amended pins (R10: "never accepts adapter saturation as success").*
    Each stored value was checked first; every one is non-finite or past
    f16, so each now expects `NonFiniteRender { layer: 0 }`:
    `cc3_boundary_controls_stay_finite_and_the_documented_extreme_overflows_to_infinity`
    (slope = power = 16 at linear 4.0: f32 +inf; the CPU monitor clamp
    to 255 is unchanged, the GPU working and monitor renders refuse);
    `cc3_monotone_nodes_never_descend_on_the_neutral_ramps`, GPU case
    `master_lift-2000_gamma4000_gain4000` (stores 115,538 at white on
    both ramps; the CPU monotone check and the other seven GPU cases are
    unchanged); `cc5_affected_pixel_containment_is_exact_on_cpu_and_gpu`
    §9.2.1 over-range GPU renders (slope = power = 16 inside the matte
    and at the unmatted 4.0 sample; the GPU over-range containment now
    runs on the finite gain grade); `zero_coverage_is_an_exact_identity`
    (non-finite inside the matte; the outside identity is read off a
    finite grade); and MO2's own `r10_refusal_names_clip_and_frame…`
    (2²⁰ now refuses at layer 0) and `review1_r10_nan_extrema…` (a NaN
    below refuses at its own layer, 0).
  - *Sampled alpha.* Every layer tests the sampled source alpha's bits for
    NaN/±inf right after the sample, before clamp, fade, crop or mask can
    erase them, stickily and on both lanes (pinned by
    `rereview_special_nonfinite_source_alpha`: Darken/Screen/Add × NaN/±inf,
    red on both lanes before the fix). The shader writes that flag in its
    own statement at the sample. Carrying it as a `bool` into the final
    flag conditions made the RTX 3090 (driver 615.71.09) stop flagging
    forced-NaN RGB operands, on the NVIDIA lane only (mechanism not
    diagnosed; lavapipe was green).
  - *ME9 restricted to a proved subset* (re-review S1). Four shared
    corners bound every rounding only where the output is multilinear in
    one sampling's weights. With two resampled layers (two opposing
    shifted ramps, `Add`), each sample may round independently, and a
    conformant nearest-8-bit result departs by 0.0039 where the shared
    corners give zero slack. The twin helper now applies the envelope only
    when at most one layer resamples distinct texels. That layer must be
    the topmost, a `Normal` pixel layer with uniform source alpha and only
    `transform`/`opacity`/`crop`/`mask` effects, with no Push backdrop
    anywhere. Anything else returns an error rather than a widening (a
    uniform source, such as a solid, never counts as resampled). Pinned by
    `rereview_me9_envelope_refuses_unproved_stacks` (sources 2/3/5,
    outputs 17/31/97/129). The WARP title evidence is outside the subset
    (non-uniform alpha), so its test now asserts the refusal. The gate
    fixture moves the title by whole pixels only (x = 20% is 32 px; the
    slide/push offsets at frames 1/3/4 are integral), so no lane needs
    envelope slack there. At those offsets GPU ≡ twin bit-exactly for
    special layers (the `Normal` adjustments included, ME12). The
    `Normal`-title frames keep the target's own store and differ by at
    most one f16 ULP — adjacent half values, e.g. GPU 0.2156982421875 vs
    twin 0.2158203125 (worst 0.00048828125, 12 of 16 frames) — within R27
    with zero slack (final verification N1, re-review N1; this corrects
    N18's "exactly" and N21's "½ ULP" wording). The solid keeps its scale and rotation.
- ME12 → §3 R9b/R10 (B1 fix round 3, re-review S2, lead ruling N17.4):
  **one storage rule.** A special layer (selectors 1–6 and 8) rounds its
  composite `α·B + (1−α)·D` to f16 round-to-nearest-even **in the shader**
  and emits that exactly representable value with alpha 1 (ME10). The
  α = 1 fixed-function store is therefore exact on every backend. The
  rounding is integer bit manipulation (exact power-of-two scaling built
  from the exponent bits, WGSL `round`, which ties to even). It does not
  rely on the target's conversion or `pack2x16float`, whose rounding
  Vulkan and WGSL leave unspecified. Subnormals share the 2⁻²⁴ quantum;
  a result past 65504 (a composite ≥ 65520) is ±inf and refuses, and
  R10's magnitude check reads the rounded value. The twin's
  `f16::from_f32` is the same RTE. `Normal` pixel stores (selector 0) and
  the Push backdrop (selector 7) keep the target's own conversion, so
  their bytes are the pre-MO2 bytes (B8; `normal_pre_post_identity`,
  `cc8_g2_sdr_identity` re-run). The G7 probe runs at the review's 60%
  again (70% kept), with and without the legacy cube. A bit-exact probe
  pins ties to even at 1 and 2048, a signed tie, subnormal ties at α = ½,
  65519.98 → 65504, and 65520 refused, on both lanes. The G7 probe and
  the bit-exact probe were red with the target's conversion.

- ME13 → §9 R28, §13 gate 10 (R28 worker stop, lead ruling N19):
  **the floors bound compositor frames; decode is tracked.** R28 put
  decode inside the floor protocol. Decode is outside MO2 and pre-dates
  it, so the floors could not be met by optimising copies on any backend.
  - *Evidence (release, 1280×720 proxy output from 1920×1080 documents,
    `FrameRenderer` playback path, sequential decode).* End to end,
    typical 1080p ran at ≈ 2 fps on both lavapipe and the RTX 3090, the
    same as at f241aa5 (494–498 ms per frame). A probe with continuous
    whole-clip tracks on the 3090 gave the cause:

    | media tracks | mean ms | pattern |
    |---|---|---|
    | 1 | 64 | 21–22 ms cached frames; ~610 ms every 16th (prefetch) |
    | 2 | 1,227 | ~1.2 s every frame |
    | 3 | 1,909 | ~1.9 s every frame |

    Mechanism: sequential prefetch holds 16 frames per source at
    1280×720×8 B (7.37 MB). Two sources need 236 MB, over
    `FRAME_CACHE_BYTE_BUDGET` (224 MiB). `reserve_cache_bytes` then evicts
    by insertion order, which throws away the other source's
    soonest-needed frames. The next frame misses, the decoder's
    continuation no longer matches, and `decode_window` seeks from the
    keyframe and re-decodes — every frame, for every source. A decode
    plus convert costs ≈ 38 ms per 1080p source frame.
  - *Amended protocol.* The absolute floors (lavapipe 8, WARP 20,
    RTX 3090 60 fps) and p95 ≤ 3× mean apply to **compositor frames with
    resident, pre-decoded sources**: render + readback + monitor encode,
    measured by `FrameRenderer::render_timed` with the renderer's frame
    cache raised to 1 GiB. Everything else is unchanged: 30 + 300 frames,
    three runs, one frame in flight, preview proxy output, and the
    ledger ceilings on every run. End-to-end preview frames (decode
    included) are a **tracked, non-gating** baseline, and so is their
    p95. The end-to-end **5% no-regression rule against f241aa5 still
    gates** (`r28_end_to_end_tracked`, pinned typical baselines). Decode,
    the cache policy and prefetch go to PF1.
  - *Pins (compositor frames, three-run mean / worst p95).*

    | backend | typical 1080p | blend_heavy 1080p | heavy 4K | floor |
    |---|---|---|---|---|
    | lavapipe (llvmpipe, LLVM 22.1.8) | 33.1 / 36.8 ms, 30.2 fps | 44.4 / 49.0 ms, 22.5 fps | 38.8 / 43.6 ms | 8 fps: holds |
    | RTX 3090 (driver 615.71.09) | 28.9 / 36.8 ms, 34.6 fps | 36.5 / 46.1 ms, 27.4 fps | 31.9 / 42.0 ms | 60 fps: **missed, pre-existing** |
    | WARP | owed (ME14) | owed (ME14) | — | 20 fps |

    The 3090 miss is recorded, not loosened. Its breakdown (blend_heavy
    per frame, instrumented probe): CPU monitor encode 25.8 ms (CC1's
    per-pixel f16 → BT.709 OETF in f32), layer upload `write_texture`
    7.8 ms (three 1280×720 RGBA16F frames), GPU passes + snapshot copies
    1.7 ms, readback copy + map 0.8 ms (a scratch build split the submit
    to time these). Everything MO2's render path touches — passes,
    snapshot copies, flag readback — totals ≈ 2.5 ms. The pre-existing
    encode and upload alone, ≈ 34 ms, exceed the 16.7 ms frame time, so
    no copy optimisation can meet the floor; the fix goes to PF1.
    The slowdown control fails on both local lanes: lavapipe with a
    312.5 ms delay (2.8 fps), the 3090 with 41.7 ms (12.6 fps).
  - *End to end (tracked; typical 1080p, three-run mean / worst p95).*

    | backend | f241aa5 | MO2 tip | Δ (5% rule) |
    |---|---|---|---|
    | lavapipe | 497.9 / 1,587 ms | 489.1 / 1,548 ms | −1.8%: holds |
    | RTX 3090 | 494.4 / 1,576 ms | 485.2 / 1,554 ms | −1.9%: holds |

    Both trees ran the same protocol code against the same pinned FFmpeg
    build (`BASELINES` in `mo2_perf_fixtures`). A decode-bound frame
    moves with code layout: on identical sources, the tip binary's mean
    ranged from −10% to +3% across three rebuilds. The pins are the
    committed build. blend_heavy end to end: 1,709 ms (3090, one run),
    tracked only, because it has no pre-MO2 equivalent.
  - *Solo (lavapipe, release).* A 1080p clip under an adjustment, with
    the adjustment soloed. The 16-sample strip took 3.8 s and the
    full-resolution pair 0.47 s. The ledger peaked at 79.1 MiB and
    returned to 0 (`r28_solo_peak_resources_and_elapsed`).
  - *Ledger peaks.* Preview-proxy runs: typical 56.3 MiB, blend_heavy
    63.3 MiB, heavy 4K 70.3 MiB. Full-resolution frames: 126.6, 142.4
    and 632.8 MiB. Ceilings are 384 / 384 / 1,536 MiB. Every MO2 frame
    resource is charged. Layer-upload staging and failed-frame release
    follow ME15, which supersedes the final verification's
    padded-upper-bound charge and unbounded flush. LUT-atlas staging and
    the bounded readback wait follow ME16, which removes the earlier
    atlas exception. The peaks above cannot move under ME15: every
    workload's rows (1280, 1920 and 3840 px × 8 B) are already
    256-aligned, so each upload's staging is the bytes charged before,
    held for the same span (to readback). Under ME16 the full-resolution
    peaks were re-measured and are unchanged. These workloads bind only
    the identity atlas, whose staging is 1 KiB.
  - *`validate()` per frame (N11-4):* 1.1–1.8 µs at 1080p, 10 µs for the
    200-clip 4K document. No revision-keyed cache is needed.
- ME14 → §9 R28, §13 gate 10 (Windows CI run 36248329932, lead rulings
  N22 and Riel's follow-up): **the WARP floor is measured by hand, not in
  hosted CI.** A shared hosted runner does not represent the floor's
  target, a Windows desktop with no GPU. The CI step is removed. The
  fallback-adapter test stays `--ignored` with its 20 fps floor
  unchanged, and the lead runs it by hand on a local Windows VM
  (`cargo test --release -p kinewright-media --lib
  blend_heavy_holds_floors_on_the_fallback_adapter -- --ignored
  --nocapture`, with `R28_ONLY=typical_1080p,blend_heavy_1080p`). The
  WARP floor stays owed until that run is pinned here.
  - *Recorded observation (hosted `windows-latest`, 4 vCPU, WARP
    "Microsoft Basic Render Driver", release, compositor frames, three
    runs; not a pin).*

    | workload | mean ms per run | worst p95 | fps | ledger peak |
    |---|---|---|---|---|
    | typical 1080p | 796.9 / 740.0 / 750.0 | 857.6 ms | 1.3–1.4 | 56.3 MiB |
    | blend_heavy 1080p | 944.2 / 941.0 / 940.4 | 1,004.4 ms | 1.1 | 63.3 MiB |

    p95 ≤ 3× mean and the ledger ceilings held; the slowdown control
    (125 ms delay, 0.9 fps) was detected. The step took 53 min (7 min of
    it the release build). Ledger peaks match lavapipe exactly.
  - *Breakdown print.* Every resident lane now also prints one
    `R28 phases` line per workload: the mean over 30 frames (after 10
    warm-up) of upload (staging and command recording), GPU passes +
    readback (submit to mapped) and CPU monitor encode. It is recorded,
    never gated, so a backend pathology shows in the log. The timing
    helpers are test-only (`compositor::phases`, `render::phases`).
    Lavapipe, typical 1080p: 7.4 / 6.3 / 18.9 ms of a 33.3 ms frame,
    which agrees with ME13's breakdown.

- ME15 → §9 R28 (re-review of the final verification, B1/B2/S2, lead
  ruling N23): **the ledger charges API-level bytes, and MO2 owns the
  layer-upload staging.** R28's "staging" is the buffers MO2 asks wgpu
  for, charged at their API size. What a backend allocator rounds that up
  to is documented here and not charged.
  - *Explicit staging.* A pixel layer's upload no longer goes through
    `queue.write_texture`, whose internal staging row pitch belongs to the
    backend (Vulkan passes the driver's
    `optimalBufferCopyRowPitchAlignment` through unclamped, so no constant
    bounds it). `upload_layer` creates one buffer per uploaded layer
    (`MAP_WRITE | COPY_SRC`, mapped at creation). Each row is padded to
    `COPY_BYTES_PER_ROW_ALIGNMENT` (256), the `copy_buffer_to_texture`
    contract on every backend. The rows are written into the mapping, the
    buffer is unmapped and charged exactly (`padded row × height`). The
    frame's encoder copies it into the pooled source texture before its
    first pass. The buffer is held in the layer's resources until
    readback and dropped with the frame. It is not reused across frames:
    reuse would need a `map_async` and a poll per frame. The uniform and
    grade `write_buffer` staging stays charged at its data size, because
    buffer writes have no row pitch. The LUT atlas's upload follows the
    same path (ME16).
  - *Backend granularity (documented, not charged).* DX12 places
    buffers on 64 KiB boundaries: on the Windows CI WARP adapter, wgpu's
    buffer counter moved 65,536 B for a 32-px-wide upload charged
    16,384 B. Vulkan implementations suballocate with their own
    alignment; lavapipe's counter delta equals the charge. These are
    allocator properties. The ledger bounds what MO2
    requests, so the counters are reported (`LEDGER_UPLOAD … backend_delta`)
    and never asserted. `reverify_b1_legal_vulkan_pitch_512` is moot:
    MO2 now chooses the pitch, so a driver's larger recommendation no
    longer changes the bytes staged.
  - *Bounded failed-frame wait.* A frame that fails after staging submits
    nothing new, registers `on_submitted_work_done`, and polls
    `PollType::Wait` for that submission with a **100 ms** timeout
    (`FAILED_FRAME_WAIT`). If the callback has run, the frame's
    resources and charges drop at once. Otherwise (a timeout, a poll
    error) the frame moves to a `retired` list, still charged. Each later
    `composite` sweeps the list and drops only frames whose callback a
    poll has since run. Teardown drops the list with the compositor.
    wgpu-core runs pending completion callbacks when a lost device's
    queue drains, so device loss releases the frames too. A malformed
    frame's refusal therefore waits at most 100 ms, and no charge leaves
    before its writes are done.
  - *Tests (default lane, all backends).*
    - `final_ledger_upload_padding_counterexample` checks the row, the
      buffer size and the charge against `(w·8)⌈256⌉ × h` for widths
      1–65 and heights 1/3/64. It also checks that the frame copies that
      buffer (G02).
    - `final_ledger_exact_resources_and_lifetime` covers exactness and
      lifetime.
    - `final_ledger_failed_frame_retains_pending_uploads`: four failed
      frames retire nothing and leave the ledger at baseline.
    - `reverify_b2_requires_bounded_wait`: the poll carries a timeout of
      at most 1 s.
    - `reverify_b2_poll_error_preserves_charges`: a Timeout keeps every
      charge and one retired frame, and a sweep with no completion keeps
      it. A later completing poll plus the next frame release it to
      baseline.
    - `reverify_b2_flush_observes_live_charges` (G08): the charges are
      live when the poll runs.
    - `reverify_me14_phases_sum_same_frame` (G09/G10): the printed phases
      partition one measured frame, each non-zero.
    - `reverify_me14_warp_floor_boundary` (G11): synthetic WARP adapter
      metadata selects the 20 fps floor and its boundary.

    Each probe was red against its mutation: padding at 128, uncharged
    staging, release before the poll, an unbounded wait, a timeout taken
    as completion, a missing sweep, a zero phase, and floor 8. Probes that
    parse `include_str!` sources normalise CRLF, so Windows checkouts
    parse them identically.
- ME16 → §9 R28 (render re-verification 2, B1/B2/S1, lead ruling N25):
  **every frame resource is charged, and every frame-path wait is
  bounded, with completion-owned charges.**
  - *Atlas staging.* A cold LUT atlas no longer uses
    `queue.write_texture`. `build_lut_atlas` writes every slot into one
    ME15 staging buffer (`staging_rows`: mapped at creation, charged
    exactly). Each lattice row (`S × 16` B) sits at the widest slot's
    pitch, padded to 256. One `copy_buffer_to_texture` per slot goes into
    the atlas's own command buffer, which is submitted at once. Queue
    order therefore puts the copy ahead of any frame that samples the
    atlas, exactly as the queue write did. The staging buffer joins the
    `retired` list with that submission's completion flag. It stays
    charged until a poll observes the flag. It is normally swept at the
    end of the frame that built it, since that frame's readback wait
    completes the copy too. Otherwise it goes at the next failed frame,
    composite or teardown. ME13's atlas exception is gone.
  - *Bounded readback.* The readback wait covers every read-back frame:
    normal frames, R10 refusals, and encode errors after readback. It now
    polls `Wait` for the frame's own submission with a **10 s** bound
    (`READBACK_WAIT`, N25). The map callback, not the poll status,
    decides completion.
    - If the callback has not run by then, the frame refuses with
      `gpu_readback_timeout`. That is a `Backend` message with a stable
      code prefix, like `lut_atlas_too_large`; a `MediaError` variant
      would need a `kinewright-core` change outside this slice. The
      frame's output, readback buffer and layer resources move to
      `retired` under the submission's completion flag.
    - A later sweep drops them exactly once, after a poll observes
      completion. Teardown also drops them.
    - A driver watchdog that fires first surfaces as device loss, whose
      map callback errors, so the frame refuses either way.
    - The staging cleanup stays at 100 ms (ME15).
    - Both waits go through one `frame_poll`, which the tests observe
      and override. These are the only waits in the frame path.
  - *Tests (default lane; NVIDIA once with `--include-ignored`, 15/15).*
    - `rev2_api_atlas_write_staging_is_charged`: the atlas charges its
      texture plus its staging, 1,152 B for the identity cube, against the
      earlier 128 B. The staging is retained until completion is
      observed, then released.
    - `rev2_runtime_refusal_readback_wait_is_bounded`: an R10 refusal's
      only wait is exactly `Wait{Some(_), Some(10 s)}`.
    - `rev2_simulated_readback_timeout_keeps_submitted_charges`: an
      injected timeout refuses with `gpu_readback_timeout`. It keeps
      every charge live at the poll (51,436 B, against a 9,100 B
      baseline) and one retired frame, and a sweep without completion
      keeps them. After a completing poll the sweep releases them; the
      ledger returns to the baseline after the next frame, and to 0 at
      teardown.
    - The five survivor probes of the re-verification:
      - `rev2_failed_flush_has_submission_index` (G05).
      - `rev2_all_staging_error_paths_have_100ms_cleanup` (G07). All 18
        staging refusals clean up with exactly one 100 ms wait and no
        readback wait.
      - `rev2_exact_100ms_cleanup_argument` (H01), which checks for
        exactly 100 ms.
      - `rev2_callback_completion_overrules_error_status` (H06).
      - `rev2_upload_pixels_rgba8_rgba16_and_copy_count` (H10). Each
        pixel layer gets one upload copy, and RGBA8 and RGBA16F pixels
        arrive exact.

    Each probe was red against its mutation. The survivor mutations were
    no submission index, cleanup only above one staged layer, a 500 ms
    wait, poll `Ok` taken as completion, and a duplicated upload copy.
    The mutations for the new rules were:
    - an uncharged atlas staging;
    - an unbounded readback wait;
    - a 20 s readback wait;
    - a timeout that releases the frame;
    - a timeout that drops the output;
    - a timeout refusal without retention.

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
without a new design. *(ME10: the over is now composited in-shader and
emitted opaque; the composite and `out_a = 1` are unchanged. ME12: the
emitted composite is already f16 round-to-nearest-even, so the store is
exact.)* Pinned by an opaque-accumulator assert (every
accumulator readback alpha ≡ 1) plus the vector. Pin a partially
transparent colour-fade source with mask and non-Normal blending.

**R10 [media] (domain).** No clamp inside blend (the CC1
no-intermediate-clamp invariant survives MO2). MO2 refuses non-finite
working values and overflow at an f16 storage boundary; it never accepts
adapter saturation as success. Detect invalid values before storage and
retain a sticky per-layer failure indication until readback, including
failures subsequently covered by another layer (ME11: every layer,
`Normal` pixel layers included). Return a typed `MediaError`
carrying offending clip and project frame through working, monitor,
delivery and proof paths; map it through the existing incident family
without adding an investigator code. Representable values use
round-to-nearest f16 storage: *(ME12)* a special layer rounds to f16
round-to-nearest-even in its shader and the store is exact; `Normal`
pixel stores and the Push backdrop keep the target's conversion
(pre-MO2 bytes). Pin `Add(40000,40000)` and
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
adjustment Push, the shifted backdrop is re-snapshotted into a second
pooled texture so `D0` stays in the first (ME2). Copy counts are ordinary
special 1, Normal Push 1, non-Normal Push 2, and any adjustment Push 2
(ME10). Ledger the second pooled snapshot. Pin transparent/masked/transformed endpoints,
OOB fallback, adjustment Push, and exact completion equality with ordinary
composition.

**R14 [media] (ABI).** `LayerParams`/WGSL grow exactly 4 words — 208 → 224
bytes asserted by an exact layout/size test: `blend_mode` selector (0 =
`Normal`, accumulator sample under guard) plus transition-coverage words
(edge, axis, on) shared by wipe/slide/push; `Push`/`Slide` offsets fold
host-side into the offset arms; output-space coverage uses the fragment's
pixel position (ME8). One pipeline, one bind-group
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
Resampled GPU≡twin values add the ME9 sub-texel envelope (ME11: only on its proved subset). R27's tolerances are MO2-specific; the future CC8 column additionally
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
all three backends and pins; a miss optimises copies, never tolerances. *(ME13: the floors bind compositor frames with resident sources; end-to-end is tracked, its 5% rule gates. ME14: WARP is measured by hand on a local Windows VM, not in hosted CI.)*

## 10 Incidents

**R29 [core] (mapping).** Zero new `IncidentCode`s; the investigator code
allowlist stays at 54. The seven R8 variants join the compile-forced
(variant → `IncidentFamily`) table as `Placement` (two on-audio-track) or
`Malformed` (five semantic), with policy/recovery rows where required.
Reused: `UnknownTransition` / `InvalidTransitionDuration`,
`SpeedOnNonMediaClip`, `EditorialRequiresMedia`, keep-outside/enable
machinery, v1/v2 refusal. Solo failures are `SoloError` (R24), not
incidents. The render entry's `MediaError::InvalidDocument` delegates
its code and evidence to the wrapped `OpError` (B1 fix G10). Pinned by
table tests + one render test per reused arm.

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
    floors). Backend + ledger tests. *(ME13: compositor frames; RTX 3090
    floor missed, pre-existing, PF1.)*
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
  any adjustment Push 2 (ME2, ME10) — stand until a floor says otherwise).
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
