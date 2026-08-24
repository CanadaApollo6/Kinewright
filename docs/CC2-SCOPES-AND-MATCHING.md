# CC2 — Scopes and matching contract

Status: completed vertical slice, 2026-08-24. This brief defines deterministic
evidence from rendered RGBA8 monitor frames, the human reference-shot workflow,
and revision-gated agent matching proposals. Measurement never changes a grade;
an agent proposal exposes the exact existing `primary_correction` operations and
remains unapplied until the caller submits them through the normal edit-plan path.

## Editor job and boundary

An editor needs to inspect a shot or a small set of project frames, choose a
geometric region, compare a candidate with a named reference, and decide what
to change. Kinewright supplies reproducible evidence for that decision. The
core engine never mutates a document, inserts a correction, chooses a preferred
look, or claims that a measured difference is an error. The editor panel uses the
same typed evidence for waveform, RGB parade, vectorscope, ROI, reference capture,
and signed comparison. `plan_shot_match` may turn recorded deltas into a bounded
starting proposal, but it returns visible operations and does not apply them.

The only currently renderable scope stage is the explicit vocabulary value
`monitoring_post_composite` (`ScopeStage::MonitoringPostComposite`). It means
the managed monitor image after the compositor and before delivery encoding.
CC2 does not expose source/pre-grade, effect-scoped, pre-composite, or
encoded-delivery scopes. A caller asking for another stage fails with a typed
`UnsupportedStage` error; it must not silently fall back to monitoring. Agent
requests use a full-resolution managed proof by default. An explicitly requested
bounded proxy is allowed for exploratory analysis only and is marked
`full_resolution: false` at every response level.

## Inputs and source truth

The pure core API measures borrowed `kinewright_core::RgbaImage` values. Every
input must satisfy all of the following:

- Width and height are non-zero `u32` values.
- The pixel buffer is exactly `width * height * 4` bytes. Overflow and mismatch
  are errors; the engine does not truncate, pad, or infer a stride.
- Each sample is `[R, G, B, A]` with 8-bit channels. A pixel with `A == 0` is
  excluded from every histogram, statistic, density grid, and clipping count.
  Every `A > 0` pixel is included at full weight; partial alpha is not an
  implicit coverage weight.
- A temporal request supplies one or more `ScopeFrame` values with an explicit
  non-negative project-frame number. Frame numbers must be unique. Input order
  is not semantic: the engine sorts by project frame and records the sorted
  identities in the result. At most 64 frames may be supplied to one
  measurement. All frames in one measurement must have the same source
  resolution.
- The engine measures the supplied raster at full source resolution. It does
  not downsample the image to make a scope request fit a configured output
  grid. Source resolution, ROI pixel resolution, `full_resolution: true`,
  alpha exclusions, and project frame identities are recorded in
  `ScopeMeasurementMetadata`.

An empty frame list, negative or duplicate frame identity, malformed image,
zero-area/empty ROI, ROI that rasterizes to no pixel, or all-transparent ROI
returns an error. No empty statistic is manufactured.

## ROI contract

`NormalizedRoi` uses unsigned basis points (`0..=10_000`) for `x`, `y`, width,
and height. Width and height are positive, and `x + width` and `y + height`
must not exceed `10_000`. The rectangle is half-open. `full_frame()` is
`(0, 0, 10_000, 10_000)`.

Conversion to pixels is deterministic and independent of floating-point
behavior:

```text
left   = floor(x                 * source_width  / 10_000)
top    = floor(y                 * source_height / 10_000)
right  = ceil((x + width)        * source_width  / 10_000)
bottom = ceil((y + height)       * source_height / 10_000)
```

The resulting `[left, right) × [top, bottom)` rectangle must contain at least
one source pixel and remains inside the source raster. A boundary exactly on a
pixel edge belongs to the half-open rectangle; a boundary between pixels uses
the ceil rule for the exclusive end so touched pixels are not silently lost.

## Bounded resolutions and output units

`ScopeResolution` controls output sizes. Every dimension is validated before
allocation and must be at least 1. The hard upper bounds are:

| Output | Default | Maximum |
| --- | ---: | ---: |
| RGB/luma histogram bins | 256 | 4,096 |
| luma waveform columns × rows | 64 × 256 | 2,048 × 1,024 |
| RGB parade columns × rows per channel | 64 × 256 | 2,048 × 1,024 |
| vectorscope side length | 256 | 1,024 |

These bounds apply to every request, including agent-provided values. Grid
length arithmetic is checked before allocation. The defaults are a stable API
default, not a claim about display pixel dimensions.

RGB and Rec.709 luma summary codes use a 16-bit fixed scale: an 8-bit code `v`
is represented as `v * 257`, from `0` through `65_535`. Luma is the integer
Rec.709 approximation already used by the core monitor scope path:

```text
luma8 = floor((54 * R + 183 * G + 19 * B) / 256)
```

Means are normalized millionths (`0..=1_000_000`), rounded to nearest integer
with half-up ties. Percentiles use nearest rank, `ceil(p * N / 100)`, over
visible samples: `first_percentile` is `p=1`, `median` is `p=50`, and
`ninety_ninth_percentile` is `p=99`. Percentiles are returned in the 16-bit
fixed code scale. These definitions avoid platform-dependent floating-point
sorting and aggregation.

## Scope outputs

`measure_scope` handles one frame; `measure_scopes` handles explicitly identified
temporal samples. Temporal aggregation concatenates all visible pixels from
the sorted frames. It does not average frame summaries, discard duplicate
content, or sample an implicit time range. `ScopeEvidence` includes:

- `ScopeMeasurementMetadata`: stage, source and sampled ROI resolutions,
  normalized and pixel ROI, full-resolution marker, project frames, total ROI
  positions, transparent positions, and visible positions.
- `ScopeStatistics`: RGB and luma mean, first, median, and 99th percentile.
- `ScopeClipping`: per-channel black and white rates in basis points of visible
  pixels. Black clipping is code `0..=1`; white clipping is `254..=255`.
  Basis points use integer floor (`count * 10_000 / visible_count`), so a rate
  is never overstated by rounding.
- `ScopeHistograms`: separate RGB and luma arrays with the requested bin count.
  A code maps to `floor(code * bins / 256)`; code 255 is always in the final
  bin.
- `LumaWaveform`: row-major density, with each source ROI column mapped to a
  configured column by floor scaling. Row zero is high code/white and the last
  row is low code/black.
- `RgbParade`: separate red, green, and blue row-major density grids using the
  same column and vertical mapping as the waveform.
- `VectorscopeDensity`: square row-major chroma density. The exact integer
  axes are `U = B - R` and `V = 2*G - R - B`, each clamped to `[-255, 255]`.
  Neutral is the centre; horizontal position maps U from left to right and
  vertical position maps V from bottom to top. No colour-space conversion or
  skin-tone preference is implied.

Density cells and histogram bins are `u64` counts. Counts and channel sums are
checked for overflow; a hostile input fails instead of wrapping.

## Reference comparison

`compare_scope_evidence` (also available as `compare_scopes` or
`ScopeEvidence::compare`) compares two already measured results. It requires
the same named stage, normalized ROI, and each configured output resolution.
Source dimensions and project-frame identities may differ and are retained as
reference/candidate metadata. A mismatch returns a typed error rather than
resampling or silently comparing unlike grids.

`ScopeComparison` records both endpoints and signed candidate-minus-reference
deltas. Positive means the candidate metric is numerically higher; negative
means it is lower. Deltas cover ROI positions, transparent and visible sample
counts, all RGB/luma summary statistics, per-channel clipping, histogram bins,
waveform cells, each parade channel, and vectorscope cells. Comparison is
evidence-only: it never proposes an exposure, white-balance, LUT, or other
operation.

## In scope for CC2

- Full-resolution-aware RGBA8 monitoring/post-composite scope measurement.
- Deterministic geometric ROI conversion and validation.
- Configurable, bounded RGB/luma histograms, luma waveform, RGB parade, and
  vectorscope density.
- Integer fixed-point means, first/median/99th percentiles, and clipping basis
  points with explicit alpha-zero exclusion.
- Deterministic aggregation over an explicitly named set of project frames.
- Reference-vs-candidate evidence with recorded signed deltas.
- Public serde/JSON-schema types suitable for agent and application inspection.
- A non-blocking editor panel with waveform, RGB parade, vectorscope, geometric
  ROI, full-raster/backend provenance, explicit reference capture, and signed
  current-minus-reference deltas. Capturing evidence has no operation side effect.
- `get_video_scopes_v2` with bounded full-resolution/default or explicit-proxy
  sampling, exact frames or a half-open temporal range, named stage, and ROI.
- Evidence-only `analyze_color_shot` and `plan_shot_match` tools with revision,
  confidence basis, assumptions, retained reference evidence, signed deltas, and
  exact unapplied `primary_correction` operations for every candidate.
- Analytic fixtures for ramps/primaries, transparent pixels, ROI boundaries,
  temporal samples, comparison signs, malformed buffers, overflow bounds, and
  empty measurements.

## Out of scope and deferrals

- Any hidden mutation, automatic correction, `auto_grade`, creative preference,
  or operation that bypasses revision gating and the existing undoable Core path.
- One-click human match application. The editor retains the reference and deltas,
  then adjusts the existing Primary correction controls explicitly; the agent can
  return the exact same typed starting operations for review and later commit.
- Source/pre-grade, effect-node, matte-scoped, pre-composite, or
  post-encode/delivery scopes. Explicit proxy evidence cannot establish full-raster
  or delivery correctness.
- HDR, RAW, log-to-working conversion, gamut mapping, calibrated-monitor
  transforms, false-colour, skin-tone target bands, and Delta E 2000. The
  supplied monitoring raster is measured as-is.
- Curves, wheels, secondaries, tracked mattes, look/LUT management, or an
  implicit reference group. CC2 matching uses only the existing primary node.
- Float-valued coordinates, alpha weighting, nondeterministic GPU reductions,
  adaptive/unbounded resolutions, implicit time ranges, frame interpolation,
  codec-specific tolerance claims, and hidden proxy substitution.

## Exit evidence

The core gate is the analytic fixture suite in `kinewright-core`: primary
channels and ramps occupy predictable histogram/parade/vectorscope cells;
transparent pixels do not affect counts; ROI boundaries map identically across
runs; temporal frame IDs are sorted and aggregated; comparison signs are
candidate-minus-reference; malformed/overflow/empty inputs fail with typed
errors; and serialized evidence round-trips without loss. App tests prove stale
async responses are rejected, project/ROI changes invalidate the right evidence,
reference comparison is signed, and measurement emits no operation. The agent
two-shot fixture records reference/candidate deltas, full-resolution provenance,
confidence/assumptions, and exact revision-gated operations while proving the
source document is unchanged; stale revisions and unsupported stages fail closed.

The remaining hands-on gate is platform smoke testing of the interactive panel on
Windows and Omarchy. Automated Linux workspace verification covers compilation,
math, state transitions, tool schemas, and the two-shot no-hidden-change contract;
it does not claim a human judgement about whether a proposed match is aesthetically
preferred.
