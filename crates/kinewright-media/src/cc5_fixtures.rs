//! Objective CC5 evidence fixtures for `docs/CC5-SECONDARIES.md` §9.
//!
//! These fixtures live inside the media crate for the same reason the CC1,
//! CC3, and CC4 fixtures do: the `Rgba16Float` working frame, the matte block
//! inside the grade buffer, and the production compositor are internal seams,
//! and the evidence has to exercise the real GPU path rather than a public
//! re-implementation of it.
//!
//! Every helper CC1 owns — provenance, the banded §6.2 linear gate, the
//! monitor code metric, the evidence artefact writer — and every helper CC3
//! owns — the §10.2 parity samples, the vacuity gate — is reused from
//! [`crate::cc1_fixtures`] and [`crate::cc3_fixtures`] rather than duplicated,
//! so a CC1 tolerance can never drift away from a CC5 fixture that claims to
//! reuse it.
//!
//! Per CC5 §9.0 rule 1 no expected value in this file is obtained by calling
//! `Matte::coverage`, `MatteWindow::weight`, `MatteQualifier::weight`,
//! `apply_color_nodes_at`, the compositor, or the shader. Expected values are
//! either literal constants transcribed from the contract tables or computed
//! by the `spec_*` functions below, which are an independent transcription of
//! §2.3, §2.4, and §2.5.
//!
//! Per CC5 §9.0 rule 7 the vacuity gate is **two-sided**:
//! [`assert_matte_containment`] requires at least
//! [`MIN_CHANGED_LINEAR_BASIS_POINTS`] of the pixels *inside* the matte to
//! change and **exactly zero** pixels outside it, `f32::to_bits`-identical.

#![allow(clippy::cast_possible_truncation)]
#![allow(clippy::cast_possible_wrap)]
#![allow(clippy::cast_precision_loss)]
#![allow(clippy::cast_sign_loss)]
#![allow(clippy::excessive_precision)]
#![allow(clippy::float_cmp)]
#![allow(clippy::items_after_statements)]
#![allow(clippy::many_single_char_names)]
#![allow(clippy::needless_pass_by_value)]
#![allow(clippy::similar_names)]
#![allow(clippy::too_many_arguments)]
#![allow(clippy::too_many_lines)]
#![allow(clippy::type_complexity)]
#![allow(clippy::uninlined_format_args)]

use std::{collections::BTreeMap, fmt::Write as _, fs, sync::Arc, time::Instant};

use half::f16;
use kinewright_core::{
    Analysis, AutomationCurve, ClipId, ColorNodeInactiveReason, Document, Effect, EffectId,
    Keyframe, KeyframeInterpolation, LutAsset, LutAssetId, LutAvailabilityKind,
    MATTE_PARAMETER_COUNT, MATTE_WINDOW_LIMIT, MatteParams, ParamValue, TimeCode,
    color_node_inactive_reason, effect_descriptor, is_matte_parameter, matte_parameter_names,
    matte_window_parameter_names,
};
use serde_json::{Value, json};

use crate::{
    COMPOSITOR_REQUIRED_STORAGE_BUFFER_BINDING_SIZE,
    COMPOSITOR_REQUIRED_STORAGE_BUFFERS_PER_SHADER_STAGE, Compositor, CompositorLayer,
    MatteRenderTarget,
    cc1_fixtures::{
        FixtureGpu, LINEAR_CPU_GPU_MAX, LINEAR_CPU_GPU_MEAN, LINEAR_CPU_GPU_P99,
        LINEAR_GATE_DOMAIN, LINEAR_GATE_IN_GAMUT, LINEAR_OVER_RANGE_MEAN, LINEAR_OVER_RANGE_P99,
        MIN_CHANGED_LINEAR_BASIS_POINTS, MONITOR_CPU_GPU_MAX, MONITOR_CPU_GPU_MEAN,
        MONITOR_CPU_GPU_P99, abs_code_diff_rgb, assert_linear_parity, assert_manifest_f32,
        assert_manifest_f64, backend_metadata, fallback_gpu, git_revision, hardware_gpu,
        linear_parity_metrics, output_hash, simple_document, working_frame,
        write_evidence_artefact,
    },
    cc3_fixtures::{cc3_parity_raster, json_hash},
    color_pipeline::{
        ColorNode, Matte, MatteWindow, apply_color_nodes_at, encode_monitor_rgba8, grade709_decode,
        resolve_color_nodes_with,
    },
    frame::WorkingFrame,
    lut_store::{LutLibrary, LutStore},
    test_support::TempDirectory,
    timeline::TransitionRenderParams,
};

/// The contract token recorded on every CC5 evidence payload.
const CC5_CONTRACT: &str = "cc5_secondaries";

/// Non-GPU fixtures still record a backend so a reader never has to guess
/// which implementation produced a number.
const CPU_REFERENCE_BACKEND: &str = "backend=kinewright_media_cpu_reference;adapter=host_f32;\
software_fallback=true;gpu_claim=false;lane=cpu_reference";
const CPU_REFERENCE_LANE: &str = "cpu_reference";

/// The CC5 §9.1 raster dimensions. Both rasters are 64 × 36, so the raster
/// aspect is exactly `16/9` and the pixel centres are `((x + 0.5)/64,
/// (y + 0.5)/36)`.
pub(crate) const CC5_RASTER_WIDTH: u32 = 64;
pub(crate) const CC5_RASTER_HEIGHT: u32 = 36;
const CC5_RASTER_PIXELS: usize = (CC5_RASTER_WIDTH * CC5_RASTER_HEIGHT) as usize;

/// The §9.1 parity raster is the 192 CC3 §10.2 samples laid out as a 16 × 12
/// grid of 4 × 3-pixel blocks.
const CC5_PARITY_BLOCK_WIDTH: u32 = 4;
const CC5_PARITY_BLOCK_HEIGHT: u32 = 3;
const CC5_PARITY_BLOCK_COLUMNS: u32 = CC5_RASTER_WIDTH / CC5_PARITY_BLOCK_WIDTH;
const CC5_PARITY_BLOCK_ROWS: u32 = CC5_RASTER_HEIGHT / CC5_PARITY_BLOCK_HEIGHT;

/// The centred rect window of §9.2.1: `center = (5000, 5000)`,
/// `half_width = half_height = 2500`, `feather = 0`.
const CENTRED_WINDOW_PIXELS: usize = 576;
/// The complement of [`CENTRED_WINDOW_PIXELS`] on the §9.1 raster.
const CENTRED_WINDOW_OUTSIDE_PIXELS: usize = CC5_RASTER_PIXELS - CENTRED_WINDOW_PIXELS;
/// `576 / 2304` as basis points, the coverage the contract derives by hand.
const CENTRED_WINDOW_BASIS_POINTS: u64 = 2_500;

/// CC5 §9.2.3: `feather = 4000` is not dyadic, so `1 ± f` each round and the
/// `D = 1.2` anchor lands one ULP off. The contract measures `1.2e-7`.
const FEATHER_NON_DYADIC_TOLERANCE: f32 = 1.5e-7;

/// CC5 §9.2.5: the qualifier anchors are fed as `grade709_decode(e)`, so the
/// round trip through the encode costs a few f32 ULP. Every anchor below is
/// measured at or under `3e-7`; this is two orders of magnitude tighter than
/// the band and softness widths it discriminates.
const QUALIFIER_ANCHOR_TOLERANCE: f32 = 1.0e-6;

/// CC5 §9.2.9: a feathered coverage byte may differ by one code between the
/// CPU reference's `round(255·m)` and the GPU's; an unfeathered one may not.
const MATTE_PROOF_FEATHERED_CODE_TOLERANCE: u8 = 1;

/// The measurement field names the §9.2.16 evidence payload carries.
///
/// The contract calls §9.2.16 *recorded evidence* whose regressions must stay
/// visible, so the names a reader greps for are part of it. Declared once,
/// asserted against the manifest by the inventory test and against the emitted
/// payload by `record_cc5_performance`.
const PERFORMANCE_EVIDENCE_FIELDS: [&str; 13] = [
    "minimum_milliseconds",
    "mean_milliseconds",
    "samples_milliseconds",
    "matte_free_milliseconds",
    "empty_stack_milliseconds",
    "empty_stack_samples_milliseconds",
    "node_stack_milliseconds",
    "readback_note",
    "soft_budget_met_by_node_stack",
    "soft_budget_milliseconds",
    "soft_budget_met",
    "gate",
    "note",
];

/// CC5 §9.2.16's soft budget: one 24 fps frame on the hardware lane. Recorded
/// evidence, never a hard gate — the software rasterizer is orders of
/// magnitude slower and a CI timing gate would be noise, not a contract.
const PERFORMANCE_SOFT_BUDGET_MILLISECONDS: f64 = 41.7;

/// Every evidence payload this suite emits. The manifest is asserted equal to
/// this list, so a fixture cannot be deleted without the manifest test failing.
const CC5_EVIDENCE_FIXTURES: [&str; 17] = [
    "cc5_rasters",
    "cc5_control_bounds",
    "cc5_containment",
    "cc5_window_geometry",
    "cc5_feather",
    "cc5_combine",
    "cc5_qualifier",
    "cc5_mix_and_invert",
    "cc5_keyframed_window",
    "cc5_gpu_cpu_parity",
    "cc5_matte_proof",
    "cc5_matte_scoped_scopes",
    "cc5_tracked_shot",
    "cc5_migration_and_mask",
    "cc5_buffer_and_limits",
    "cc5_performance",
    "cc5_skin_and_product",
];

// ---------------------------------------------------------------------------
// The independent transcription of CC5 §2.3, §2.4, and §2.5.
//
// Nothing below calls `MatteWindow`, `MatteQualifier`, `Matte`, the compositor,
// or the shader. The algorithms are the contract's pseudocode, transcribed by
// hand, so every geometry, feather, and qualifier expectation compares two
// implementations of the written contract rather than one implementation with
// itself.
// ---------------------------------------------------------------------------

/// The exact `smoothstep(A, B, x) = t·t·(3 − 2t)` of CC5 §2.3, in f64.
fn spec_smoothstep_f64(start: f64, end: f64, value: f64) -> f64 {
    let t = ((value - start) / (end - start)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// The §2.3 normalized distance field `D` of one window at `uv`, in f64.
///
/// `shape_ellipse` selects `sqrt(n.x² + n.y²)` over `max(|n.x|, |n.y|)`; the
/// half-extents are fractions of width and of height respectively, so the
/// aspect factor cancels at `θ = 0` exactly as the contract states.
fn spec_window_distance_f64(window: &WindowSpec, uv: [f64; 2], aspect: f64) -> f64 {
    let hw = window.hw as f64 / 10_000.0;
    let hh = window.hh as f64 / 10_000.0;
    if hw <= 0.0 || hh <= 0.0 {
        return f64::INFINITY;
    }
    let theta = (window.rotation as f64 / 100.0).to_radians();
    // The host solves `(cosT, sinT)` in f64 and rounds once to f32; the
    // reference and the shader consume that rounded pair, so this
    // transcription rounds at the same place.
    let cos_t = f64::from(theta.cos() as f32);
    let sin_t = f64::from(theta.sin() as f32);
    let dx = (uv[0] - window.cx as f64 / 10_000.0) * aspect;
    let dy = uv[1] - window.cy as f64 / 10_000.0;
    let qx = dx * cos_t + dy * sin_t;
    let qy = -dx * sin_t + dy * cos_t;
    let nx = qx / (hw * aspect);
    let ny = qy / hh;
    if window.shape == SHAPE_ELLIPSE {
        nx.hypot(ny)
    } else {
        nx.abs().max(ny.abs())
    }
}

/// The §2.3 per-window weight, including the window's own invert, in f64.
fn spec_window_weight_f64(window: &WindowSpec, uv: [f64; 2], aspect: f64) -> f64 {
    let distance = spec_window_distance_f64(window, uv, aspect);
    let feather = window.feather as f64 / 10_000.0;
    let weight = if feather <= 0.0 {
        if distance <= 1.0 { 1.0 } else { 0.0 }
    } else {
        1.0 - spec_smoothstep_f64(1.0 - feather, 1.0 + feather, distance)
    };
    if window.invert == 1 {
        1.0 - weight
    } else {
        weight
    }
}

/// The §2.3 combined geometric weight of a window list, in f64.
fn spec_window_combination_f64(
    windows: &[WindowSpec],
    union: bool,
    uv: [f64; 2],
    aspect: f64,
) -> f64 {
    let mut combined: Option<f64> = None;
    for window in windows {
        let weight = spec_window_weight_f64(window, uv, aspect);
        combined = Some(match combined {
            None => weight,
            Some(previous) if union => previous.max(weight),
            Some(previous) => previous.min(weight),
        });
    }
    combined.unwrap_or(1.0)
}

/// The pixel-centre uv of raster index `index` on the §9.1 rasters, in f64.
///
/// CC5 §3.4: fixtures evaluate at `((x + 0.5)/W, (y + 0.5)/H)`, matching the
/// rasterizer's `@builtin(position)` convention.
fn spec_pixel_centre_uv_f64(index: usize) -> [f64; 2] {
    let width = CC5_RASTER_WIDTH as usize;
    let x = (index % width) as f64;
    let y = (index / width) as f64;
    [
        (x + 0.5) / f64::from(CC5_RASTER_WIDTH),
        (y + 0.5) / f64::from(CC5_RASTER_HEIGHT),
    ]
}

/// The §9.1 raster aspect `a = W / H`, in f64.
const SPEC_RASTER_ASPECT_F64: f64 = CC5_RASTER_WIDTH as f64 / CC5_RASTER_HEIGHT as f64;

// --- the CC3 §2.1 `grade709` transfer pair, transcribed for CC5 §2.4 -------

const SPEC_GRADE709_ALPHA: f64 = 1.099_296_8;
const SPEC_GRADE709_BETA: f64 = 0.018_053_969;
const SPEC_GRADE709_BETA_ENCODED: f64 = 0.081_242_86;
const SPEC_GRADE709_K: f64 = 0.099_296_8;
const SPEC_GRADE709_SLOPE: f64 = 4.5;
const SPEC_GRADE709_EXPONENT: f64 = 0.45;
const SPEC_GRADE709_INVERSE_EXPONENT: f64 = 2.222_222_3;

/// `sgn` with `sgn(0) = 0`, the CC3 definition WGSL's `sign` also matches.
fn spec_sign_f64(value: f64) -> f64 {
    if value > 0.0 {
        1.0
    } else if value < 0.0 {
        -1.0
    } else {
        0.0
    }
}

fn spec_grade709_encode_f64(x: f64) -> f64 {
    let sign = spec_sign_f64(x);
    let magnitude = x.abs();
    if magnitude < SPEC_GRADE709_BETA {
        sign * SPEC_GRADE709_SLOPE * magnitude
    } else {
        sign * (SPEC_GRADE709_ALPHA * magnitude.powf(SPEC_GRADE709_EXPONENT) - SPEC_GRADE709_K)
    }
}

/// The exact analytic inverse of [`spec_grade709_encode_f64`].
///
/// Retained beside its inverse so the transcription of CC3 §2.1 is complete
/// and the round trip below can be checked by inspection; CC5's own anchors
/// only ever need the encode leg.
#[allow(dead_code)]
fn spec_grade709_decode_f64(e: f64) -> f64 {
    let sign = spec_sign_f64(e);
    let magnitude = e.abs();
    if magnitude < SPEC_GRADE709_BETA_ENCODED {
        sign * magnitude / SPEC_GRADE709_SLOPE
    } else {
        sign * ((magnitude + SPEC_GRADE709_K) / SPEC_GRADE709_ALPHA)
            .powf(SPEC_GRADE709_INVERSE_EXPONENT)
    }
}

/// The CC3 §2.2 `color_wheels` node, transcribed in **f32** ops.
///
/// f32 rather than f64 on purpose: §9.2.1's over-range half needs to establish
/// that the node's output really is non-finite in the working precision, and
/// an f64 transcription would never overflow. This is a premise check, not an
/// expected output — the expected output there is "the input, unchanged".
fn spec_wheels_apply_f32(slope: f32, offset: f32, power: f32, x: f32) -> f32 {
    let encoded = spec_grade709_encode_f32(x);
    let shifted = encoded * slope + offset;
    let sign = if shifted > 0.0 {
        1.0
    } else if shifted < 0.0 {
        -1.0
    } else {
        0.0
    };
    spec_grade709_decode_f32(sign * shifted.abs().powf(power))
}

fn spec_grade709_encode_f32(x: f32) -> f32 {
    let sign = spec_sign_f64(f64::from(x)) as f32;
    let magnitude = x.abs();
    if magnitude < SPEC_GRADE709_BETA as f32 {
        sign * SPEC_GRADE709_SLOPE as f32 * magnitude
    } else {
        sign * ((SPEC_GRADE709_ALPHA as f32) * magnitude.powf(SPEC_GRADE709_EXPONENT as f32)
            - SPEC_GRADE709_K as f32)
    }
}

fn spec_grade709_decode_f32(e: f32) -> f32 {
    let sign = spec_sign_f64(f64::from(e)) as f32;
    let magnitude = e.abs();
    if magnitude < SPEC_GRADE709_BETA_ENCODED as f32 {
        sign * magnitude / SPEC_GRADE709_SLOPE as f32
    } else {
        sign * ((magnitude + SPEC_GRADE709_K as f32) / SPEC_GRADE709_ALPHA as f32)
            .powf(SPEC_GRADE709_INVERSE_EXPONENT as f32)
    }
}

/// The CC5 §2.4 selectors of one linear triple, in f64: `(S, Y, H)` with `H`
/// absent exactly when `C == 0`.
fn spec_selectors_f64(linear_rgb: [f32; 3]) -> (f64, f64, Option<f64>) {
    let clamped =
        linear_rgb.map(|value| spec_grade709_encode_f64(f64::from(value)).clamp(0.0, 1.0));
    let [red, green, blue] = clamped;
    let maximum = red.max(green).max(blue);
    let minimum = red.min(green).min(blue);
    let chroma = maximum - minimum;
    let saturation = if maximum <= 0.0 {
        0.0
    } else {
        chroma / maximum
    };
    let luma = 0.2126 * red + 0.7152 * green + 0.0722 * blue;
    // The written branch order is normative: red before green before blue.
    let hue = if chroma == 0.0 {
        None
    } else if maximum == red {
        Some(60.0 * ((green - blue) / chroma).rem_euclid(6.0))
    } else if maximum == green {
        Some(60.0 * ((blue - red) / chroma + 2.0))
    } else {
        Some(60.0 * ((red - green) / chroma + 4.0))
    };
    (saturation, luma, hue)
}

/// One CC5 §2.4 band: `min` of the two shoulders, never a product.
fn spec_band_f64(value: f64, low: f64, high: f64, softness: f64) -> f64 {
    if low > high {
        return 0.0;
    }
    if softness <= 0.0 {
        return if low <= value && value <= high {
            1.0
        } else {
            0.0
        };
    }
    spec_smoothstep_f64(low - softness, low, value)
        .min(1.0 - spec_smoothstep_f64(high, high + softness, value))
}

/// The CC5 §2.4 qualifier weight of one linear triple, in f64.
fn spec_qualifier_weight_f64(qualifier: &QualifierSpec, linear_rgb: [f32; 3]) -> f64 {
    let (saturation, luma, hue) = spec_selectors_f64(linear_rgb);
    let hue_center = qualifier.hue_center as f64 / 100.0;
    let hue_width = qualifier.hue_width as f64 / 100.0;
    let hue_softness = qualifier.hue_softness as f64 / 100.0;
    let hue_weight = if hue_width >= 180.0 {
        1.0
    } else if let Some(hue) = hue {
        let separation = (hue - hue_center).abs();
        let separation = separation.min(360.0 - separation);
        if hue_softness <= 0.0 {
            if separation <= hue_width { 1.0 } else { 0.0 }
        } else {
            1.0 - spec_smoothstep_f64(hue_width, hue_width + hue_softness, separation)
        }
    } else {
        0.0
    };
    hue_weight
        * spec_band_f64(
            saturation,
            qualifier.sat_low as f64 / 10_000.0,
            qualifier.sat_high as f64 / 10_000.0,
            qualifier.sat_softness as f64 / 10_000.0,
        )
        * spec_band_f64(
            luma,
            qualifier.luma_low as f64 / 10_000.0,
            qualifier.luma_high as f64 / 10_000.0,
            qualifier.luma_softness as f64 / 10_000.0,
        )
}

/// The CC5 §2.5 resolved coverage of one matte at one pixel, in f64.
fn spec_coverage_f64(matte: &MatteSpec, uv: [f64; 2], aspect: f64, rgb_in: [f32; 3]) -> f64 {
    let windows = spec_window_combination_f64(&matte.windows, matte.combine == 0, uv, aspect);
    let qualifier = matte.qualifier.as_ref().map_or(1.0, |qualifier| {
        spec_qualifier_weight_f64(qualifier, rgb_in)
    });
    let raw = windows * qualifier;
    let inverted = if matte.invert == 1 { 1.0 - raw } else { raw };
    inverted * (matte.mix as f64 / 10_000.0)
}

// ---------------------------------------------------------------------------
// Matte specifications: the stored integers, in one place.
//
// A spec is *data*, not behaviour: it is both the source of the `matte_*`
// parameters written onto an effect and the input to the `spec_*` reference
// above, so a fixture states one window once and the two implementations it
// compares can never be handed different windows.
// ---------------------------------------------------------------------------

/// `matte_window{j}_shape_token` for a rectangle (CC5 §2.2).
const SHAPE_RECT: i64 = 1;
/// `matte_window{j}_shape_token` for an ellipse (CC5 §2.2).
const SHAPE_ELLIPSE: i64 = 2;
/// `matte_combine_token` for union (CC5 §2.3).
const COMBINE_UNION: i64 = 0;
/// `matte_combine_token` for intersection (CC5 §2.3).
const COMBINE_INTERSECTION: i64 = 1;
/// `matte_mix_basis_points` at full strength.
const MIX_FULL: i64 = 10_000;

/// One geometric window's eight stored integers (CC5 §2.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct WindowSpec {
    shape: i64,
    cx: i64,
    cy: i64,
    hw: i64,
    hh: i64,
    rotation: i64,
    feather: i64,
    invert: i64,
}

impl WindowSpec {
    /// The §9.2.1 window: centred, quarter-frame half-extents, hard edge.
    const CENTRED: Self = Self {
        shape: SHAPE_RECT,
        cx: 5_000,
        cy: 5_000,
        hw: 2_500,
        hh: 2_500,
        rotation: 0,
        feather: 0,
        invert: 0,
    };

    /// The §9.2.2 pixel-square window: `hw·a == hh == 0.2`, i.e. 7.2 px each
    /// way on a 64 × 36 raster.
    const PIXEL_SQUARE: Self = Self {
        hw: 1_125,
        hh: 2_000,
        ..Self::CENTRED
    };

    const fn with_centre(self, cx: i64, cy: i64) -> Self {
        Self { cx, cy, ..self }
    }

    const fn with_feather(self, feather: i64) -> Self {
        Self { feather, ..self }
    }

    const fn with_rotation(self, rotation: i64) -> Self {
        Self { rotation, ..self }
    }

    const fn with_shape(self, shape: i64) -> Self {
        Self { shape, ..self }
    }

    const fn inverted(self) -> Self {
        Self { invert: 1, ..self }
    }

    /// The eight `matte_window{j}_*` parameters this window stores.
    ///
    /// The names come from Core's generated table, in descriptor order, so a
    /// renamed parameter is a compile-time-visible mismatch rather than a
    /// silently ignored fixture value.
    fn parameters(&self, index: usize) -> Vec<(String, i64)> {
        let names = matte_window_parameter_names(index)
            .unwrap_or_else(|| panic!("window {index} is inside MATTE_WINDOW_LIMIT"));
        let values = [
            self.shape,
            self.cx,
            self.cy,
            self.hw,
            self.hh,
            self.rotation,
            self.feather,
            self.invert,
        ];
        // The descriptor order is normative; assert it rather than trust it,
        // because these two lists are zipped positionally.
        const SUFFIXES: [&str; 8] = [
            "shape_token",
            "center_x_basis_points",
            "center_y_basis_points",
            "half_width_basis_points",
            "half_height_basis_points",
            "rotation_centidegrees",
            "feather_basis_points",
            "invert",
        ];
        for (name, suffix) in names.iter().zip(SUFFIXES) {
            assert!(
                name.ends_with(suffix),
                "CC5 §2.2 window parameter order changed: {name} does not end with {suffix}"
            );
            assert!(
                name.starts_with(&format!("matte_window{index}_")),
                "window {index} parameter {name} is not owned by that window"
            );
        }
        names
            .iter()
            .zip(values)
            .map(|(name, value)| ((*name).to_owned(), value))
            .collect()
    }
}

/// The nine qualifier scalars plus the enable flag (CC5 §2.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct QualifierSpec {
    hue_center: i64,
    hue_width: i64,
    hue_softness: i64,
    sat_low: i64,
    sat_high: i64,
    sat_softness: i64,
    luma_low: i64,
    luma_high: i64,
    luma_softness: i64,
}

impl QualifierSpec {
    /// Every leg wide open: the hue leg disabled at its `18000` neutral, both
    /// bands full, no softness.
    const NEUTRAL: Self = Self {
        hue_center: 0,
        hue_width: 18_000,
        hue_softness: 0,
        sat_low: 0,
        sat_high: 10_000,
        sat_softness: 0,
        luma_low: 0,
        luma_high: 10_000,
        luma_softness: 0,
    };

    fn parameters(&self) -> Vec<(String, i64)> {
        vec![
            ("matte_qualifier_enabled".to_owned(), 1),
            ("matte_hue_center_centidegrees".to_owned(), self.hue_center),
            ("matte_hue_width_centidegrees".to_owned(), self.hue_width),
            (
                "matte_hue_softness_centidegrees".to_owned(),
                self.hue_softness,
            ),
            ("matte_saturation_low_basis_points".to_owned(), self.sat_low),
            (
                "matte_saturation_high_basis_points".to_owned(),
                self.sat_high,
            ),
            (
                "matte_saturation_softness_basis_points".to_owned(),
                self.sat_softness,
            ),
            ("matte_luma_low_basis_points".to_owned(), self.luma_low),
            ("matte_luma_high_basis_points".to_owned(), self.luma_high),
            (
                "matte_luma_softness_basis_points".to_owned(),
                self.luma_softness,
            ),
        ]
    }
}

/// One node's whole matte: the §2.2 stored integers, as data.
#[derive(Debug, Clone, PartialEq, Eq)]
struct MatteSpec {
    windows: Vec<WindowSpec>,
    combine: i64,
    invert: i64,
    mix: i64,
    qualifier: Option<QualifierSpec>,
}

impl MatteSpec {
    /// An enabled matte carrying exactly one window.
    fn window(window: WindowSpec) -> Self {
        Self {
            windows: vec![window],
            combine: COMBINE_UNION,
            invert: 0,
            mix: MIX_FULL,
            qualifier: None,
        }
    }

    /// An enabled matte carrying only a qualifier.
    fn qualifier(qualifier: QualifierSpec) -> Self {
        Self {
            windows: Vec::new(),
            combine: COMBINE_UNION,
            invert: 0,
            mix: MIX_FULL,
            qualifier: Some(qualifier),
        }
    }

    fn with_windows(mut self, windows: Vec<WindowSpec>, combine: i64) -> Self {
        self.windows = windows;
        self.combine = combine;
        self
    }

    fn with_qualifier(mut self, qualifier: QualifierSpec) -> Self {
        self.qualifier = Some(qualifier);
        self
    }

    fn with_mix(mut self, mix: i64) -> Self {
        self.mix = mix;
        self
    }

    fn inverted(mut self) -> Self {
        self.invert = 1;
        self
    }

    /// Every `matte_*` parameter this matte stores, master switch included.
    ///
    /// Windows past `matte_window_count` are deliberately **not** written, so
    /// a fixture that forgets to raise the count cannot accidentally pass.
    fn parameters(&self) -> Vec<(String, i64)> {
        assert!(
            self.windows.len() <= MATTE_WINDOW_LIMIT,
            "CC5 §2.2 allows at most {MATTE_WINDOW_LIMIT} windows"
        );
        let mut parameters = vec![
            ("matte_enabled".to_owned(), 1_i64),
            ("matte_window_count".to_owned(), self.windows.len() as i64),
            ("matte_combine_token".to_owned(), self.combine),
            ("matte_invert".to_owned(), self.invert),
            ("matte_mix_basis_points".to_owned(), self.mix),
        ];
        for (index, window) in self.windows.iter().enumerate() {
            parameters.extend(window.parameters(index));
        }
        if let Some(qualifier) = &self.qualifier {
            parameters.extend(qualifier.parameters());
        }
        parameters
    }

    /// This matte's resolved coverage at one pixel centre, from the
    /// independent f64 transcription.
    fn coverage_at(&self, index: usize, rgb_in: [f32; 3]) -> f64 {
        spec_coverage_f64(
            self,
            spec_pixel_centre_uv_f64(index),
            SPEC_RASTER_ASPECT_F64,
            rgb_in,
        )
    }

    /// This matte's resolved coverage at every raster pixel, from the
    /// independent transcription.
    fn coverage_values(&self, raster: &[[f32; 3]]) -> Vec<f64> {
        (0..raster.len())
            .map(|index| self.coverage_at(index, raster[index]))
            .collect()
    }

    /// Whether every coverage value this matte can produce is exactly `0` or
    /// exactly `1`: no feather, no softness, and full mix.
    ///
    /// CC5 §9.2.9 allows a feathered coverage byte to differ by one code
    /// between the two implementations and allows a hard-edged one none, so
    /// this predicate is what selects the tolerance.
    fn is_hard_edged(&self) -> bool {
        self.mix == MIX_FULL
            && self.windows.iter().all(|window| window.feather == 0)
            && self.qualifier.is_none_or(|qualifier| {
                qualifier.hue_softness == 0
                    && qualifier.sat_softness == 0
                    && qualifier.luma_softness == 0
            })
    }

    /// The set of raster pixels this matte touches at all (`m > 0`), from the
    /// independent transcription — the population CC5 §9.0.7's two-sided gate
    /// calls "inside".
    fn covered_pixels(&self, raster: &[[f32; 3]]) -> Vec<bool> {
        (0..raster.len())
            .map(|index| self.coverage_at(index, raster[index]) > 0.0)
            .collect()
    }
}

// ---------------------------------------------------------------------------
// CC5 §9.1: the two rasters.
// ---------------------------------------------------------------------------

/// The §9.1 containment raster: every channel in `[0.05, 0.95]`, strictly
/// varying in `x`, in `y`, and in both.
///
/// **No channel is ever 0**, which is required: CC3's raster contains exact
/// zeros that a gain node leaves unchanged, which would make "exactly 0
/// outside changed" pass for the wrong reason.
pub(crate) fn cc5_field_raster() -> Vec<[f32; 3]> {
    let mut samples = Vec::with_capacity(CC5_RASTER_PIXELS);
    for y in 0..CC5_RASTER_HEIGHT {
        for x in 0..CC5_RASTER_WIDTH {
            let fx = f64::from(x) / f64::from(CC5_RASTER_WIDTH - 1);
            let fy = f64::from(y) / f64::from(CC5_RASTER_HEIGHT - 1);
            samples.push([
                (0.05 + 0.9 * fx) as f32,
                (0.05 + 0.9 * fy) as f32,
                (0.05 + 0.45 * (fx + fy)) as f32,
            ]);
        }
    }
    samples
}

/// The §9.1 parity raster: the CC3 §10.2 192 samples as a 16 × 12 grid of
/// 4 × 3-pixel blocks.
pub(crate) fn cc5_parity_raster() -> Vec<[f32; 3]> {
    let samples = cc3_parity_raster();
    assert_eq!(
        samples.len(),
        (CC5_PARITY_BLOCK_COLUMNS * CC5_PARITY_BLOCK_ROWS) as usize,
        "the CC3 §10.2 sample count must tile the CC5 §9.1 raster exactly"
    );
    let mut raster = Vec::with_capacity(CC5_RASTER_PIXELS);
    for y in 0..CC5_RASTER_HEIGHT {
        for x in 0..CC5_RASTER_WIDTH {
            let block_x = x / CC5_PARITY_BLOCK_WIDTH;
            let block_y = y / CC5_PARITY_BLOCK_HEIGHT;
            raster.push(samples[(block_y * CC5_PARITY_BLOCK_COLUMNS + block_x) as usize]);
        }
    }
    raster
}

/// The §9.2.1 over-range raster variant: the field raster with a `−0.0` and an
/// over-range `4.0` sample planted **outside** the centred window.
///
/// The two planted pixels are named rather than searched for, so the fixture
/// can assert their exact raster positions and their side of the matte.
const NEGATIVE_ZERO_PIXEL: (u32, u32) = (2, 2);
const OVER_RANGE_PIXEL: (u32, u32) = (61, 33);
/// A genuine negative sample, also outside the window: `−0.0` alone cannot
/// prove a negative survived the GPU upload, because the working surface
/// normalises it to `+0.0`.
const NEGATIVE_PIXEL: (u32, u32) = (2, 33);

fn raster_index(pixel: (u32, u32)) -> usize {
    (pixel.1 * CC5_RASTER_WIDTH + pixel.0) as usize
}

fn cc5_over_range_raster() -> Vec<[f32; 3]> {
    let mut raster = cc5_field_raster();
    raster[raster_index(NEGATIVE_ZERO_PIXEL)] = [-0.0, 0.25, 0.5];
    raster[raster_index(OVER_RANGE_PIXEL)] = [4.0, 4.0, 4.0];
    raster[raster_index(NEGATIVE_PIXEL)] = [-0.25, 0.25, 0.5];
    raster
}

fn frame_of(raster: &[[f32; 3]]) -> WorkingFrame {
    working_frame(CC5_RASTER_WIDTH, CC5_RASTER_HEIGHT, raster)
}

const CC5_RESOLUTION: (u32, u32) = (CC5_RASTER_WIDTH, CC5_RASTER_HEIGHT);

// ---------------------------------------------------------------------------
// Effects.
// ---------------------------------------------------------------------------

/// A two-entry LUT library: asset 1 is an identity lattice for the technical
/// input transform, asset 2 a mild non-identity look.
///
/// CC5 does not change CC4's LUT contract, so the lattices are deliberately
/// trivial: their job here is to make the five-kind stack of §9.2.8 real, not
/// to re-gate CC4's interpolation.
fn fixture_luts(directory: &TempDirectory) -> LutLibrary {
    fn cube_text(scale: [f64; 3]) -> String {
        let mut text = String::from("LUT_3D_SIZE 2\n");
        for blue in 0..2 {
            for green in 0..2 {
                for red in 0..2 {
                    let sample = [
                        f64::from(red) * scale[0],
                        f64::from(green) * scale[1],
                        f64::from(blue) * scale[2],
                    ];
                    let _ = writeln!(text, "{:.6} {:.6} {:.6}", sample[0], sample[1], sample[2]);
                }
            }
        }
        text
    }
    let store = LutStore::for_project(&directory.path("project.kinewright"))
        .expect("a temporary project path derives a store root");
    let mut assets: Vec<LutAsset> = Vec::new();
    for (index, scale) in [[1.0, 1.0, 1.0], [0.9, 1.0, 0.8]].into_iter().enumerate() {
        let source = directory.path(&format!("cc5-look-{index}.cube"));
        fs::write(&source, cube_text(scale)).expect("the fixture LUT is written");
        let import = store
            .import_lut_asset(&source)
            .expect("the fixture LUT imports");
        assets.push(import.into_lut_asset(LutAssetId(index as u64 + 1)));
    }
    let (library, statuses) = LutLibrary::build(&assets, Some(&store));
    for (id, status) in &statuses {
        assert_eq!(
            status.kind,
            LutAvailabilityKind::Verified,
            "fixture asset {} was not verified: {status:?}",
            id.0
        );
    }
    library
}

/// The fixture asset: a 64 × 36 source at the §9.1 raster size, so a document
/// built from it renders at the raster the hand-derived sets were computed on.
fn cc5_asset() -> kinewright_core::MediaAsset {
    kinewright_core::MediaAsset {
        id: kinewright_core::AssetId(1),
        path: std::path::PathBuf::from("cc5-fixture.mp4"),
        name: "cc5 fixture".to_owned(),
        duration: TimeCode(100),
        fps: kinewright_core::Rational::new(25, 1).expect("cc5 fixture fps"),
        kind: kinewright_core::MediaKind::Video,
        resolution: Some(CC5_RESOLUTION),
        source_fingerprint: kinewright_core::MediaSourceFingerprint::default(),
        color_description: kinewright_core::ColorDescription::default(),
    }
}

fn cc5_document() -> Document {
    simple_document(cc5_asset(), CC5_RESOLUTION)
}

fn color_node_effect(id: u64, name: &str, parameters: Vec<(String, i64)>) -> Effect {
    Effect {
        id: EffectId(id),
        name: name.to_owned(),
        parameters: parameters
            .into_iter()
            .map(|(name, value)| (name, ParamValue::Integer(value)))
            .collect(),
        keyframes: BTreeMap::new(),
    }
}

/// The §9.2.1 node: `color_wheels` with `gain_master_thousandths = 1500`.
///
/// A master gain is chosen because it moves *every* channel of every
/// non-black pixel, which is what makes "changed inside" measurable and
/// "unchanged outside" meaningful on the §9.1 field raster.
fn gain_wheels(id: u64, gain_thousandths: i64, matte: Option<&MatteSpec>) -> Effect {
    let mut parameters = vec![("gain_master_thousandths".to_owned(), gain_thousandths)];
    if let Some(matte) = matte {
        parameters.extend(matte.parameters());
    }
    color_node_effect(id, "color_wheels", parameters)
}

/// The §9.2.1 over-range node: gain and gamma at their descriptor maxima on
/// every channel, so `slope = power = 16` and a `4.0` input drives the node
/// output past `f32::MAX`.
fn overflow_wheels(id: u64, matte: Option<&MatteSpec>) -> Effect {
    let mut parameters = Vec::new();
    for channel in ["master", "red", "green", "blue"] {
        parameters.push((format!("gain_{channel}_thousandths"), 4_000));
        parameters.push((format!("gamma_{channel}_thousandths"), 4_000));
    }
    if let Some(matte) = matte {
        parameters.extend(matte.parameters());
    }
    color_node_effect(id, "color_wheels", parameters)
}

// ---------------------------------------------------------------------------
// CPU reference and GPU rendering.
// ---------------------------------------------------------------------------

fn cpu_nodes(effects: &[Effect]) -> Vec<ColorNode> {
    crate::color_pipeline::resolve_color_nodes(effects).expect("CC5 fixture stack must resolve")
}

fn cpu_nodes_with(effects: &[Effect], library: &LutLibrary) -> Vec<ColorNode> {
    resolve_color_nodes_with(effects, library).expect("CC5 fixture stack must resolve")
}

/// The CC5 §3.4 pixel-centre uv of raster index `index`.
fn pixel_centre_uv(frame: &WorkingFrame, index: usize) -> [f32; 2] {
    let width = frame.width.max(1) as usize;
    [
        ((index % width) as f32 + 0.5) / frame.width.max(1) as f32,
        ((index / width) as f32 + 0.5) / frame.height.max(1) as f32,
    ]
}

/// The output raster aspect `a = W / H` the host supplies (CC5 §3.2).
fn raster_aspect(frame: &WorkingFrame) -> f32 {
    frame.width.max(1) as f32 / frame.height.max(1) as f32
}

/// The production CPU reference in the linear working domain, including the
/// normative `Rgba16Float` storage quantization.
fn cpu_reference_linear(frame: &WorkingFrame, nodes: &[ColorNode]) -> Vec<f32> {
    let aspect = raster_aspect(frame);
    frame
        .pixels
        .as_chunks::<4>()
        .0
        .iter()
        .enumerate()
        .flat_map(|(index, rgba)| {
            let output = apply_color_nodes_at(
                nodes,
                [rgba[0].to_f32(), rgba[1].to_f32(), rgba[2].to_f32()],
                pixel_centre_uv(frame, index),
                aspect,
            );
            output
                .into_iter()
                .map(|value| f16::from_f32(value).to_f32())
                .chain(std::iter::once(f16::from_f32(rgba[3].to_f32()).to_f32()))
        })
        .collect()
}

fn cpu_reference_monitor(frame: &WorkingFrame, nodes: &[ColorNode]) -> Vec<u8> {
    let aspect = raster_aspect(frame);
    frame
        .pixels
        .as_chunks::<4>()
        .0
        .iter()
        .enumerate()
        .flat_map(|(index, rgba)| {
            let output = apply_color_nodes_at(
                nodes,
                [rgba[0].to_f32(), rgba[1].to_f32(), rgba[2].to_f32()],
                pixel_centre_uv(frame, index),
                aspect,
            );
            let quantized = output.map(|value| f16::from_f32(value).to_f32());
            encode_monitor_rgba8([quantized[0], quantized[1], quantized[2], rgba[3].to_f32()])
        })
        .collect()
}

fn gpu_linear(
    compositor: &Compositor,
    frame: &WorkingFrame,
    effects: &[Effect],
    library: Option<&LutLibrary>,
) -> Vec<f32> {
    compositor
        .render_working_with_luts(
            CC5_RESOLUTION,
            &[CompositorLayer {
                frame,
                effects,
                transition: TransitionRenderParams::default(),
            }],
            library,
        )
        .expect("production GPU working-surface readback")
        .pixels
}

fn gpu_monitor(
    compositor: &Compositor,
    frame: &WorkingFrame,
    effects: &[Effect],
    library: Option<&LutLibrary>,
) -> Vec<u8> {
    compositor
        .render_monitor_with_luts(
            CC5_RESOLUTION,
            &[CompositorLayer {
                frame,
                effects,
                transition: TransitionRenderParams::default(),
            }],
            &kinewright_core::ColorContext::sdr_rec709().monitoring,
            library,
        )
        .expect("production GPU compositor should render the CC5 fixture")
        .rgba
        .as_ref()
        .clone()
}

/// The GPU coverage raster of one node's matte, one byte per pixel.
fn gpu_coverage(
    compositor: &Compositor,
    frame: &WorkingFrame,
    effects: &[Effect],
    effect: EffectId,
) -> Vec<u8> {
    compositor
        .render_matte(
            CC5_RESOLUTION,
            &[CompositorLayer {
                frame,
                effects,
                transition: TransitionRenderParams::default(),
            }],
            None,
            MatteRenderTarget {
                layer_index: 0,
                clip: ClipId(1),
                effect,
            },
        )
        .expect("production GPU matte coverage readback")
}

// ---------------------------------------------------------------------------
// CC5 §9.0 rule 7: the two-sided containment gate.
// ---------------------------------------------------------------------------

/// Counts of changed RGB samples on either side of a matte.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ContainmentCounts {
    inside_pixels: usize,
    outside_pixels: usize,
    inside_changed_pixels: usize,
    outside_changed_pixels: usize,
    inside_changed_samples: u64,
    inside_samples: u64,
}

impl ContainmentCounts {
    fn as_json(self) -> Value {
        json!({
            "inside_pixels": self.inside_pixels,
            "outside_pixels": self.outside_pixels,
            "inside_changed_pixels": self.inside_changed_pixels,
            "outside_changed_pixels": self.outside_changed_pixels,
            "inside_changed_basis_points":
                self.inside_changed_samples * 10_000 / self.inside_samples.max(1),
        })
    }
}

/// Compare two linear working rasters against a matte's covered set.
///
/// The comparison is `f32::to_bits`, not an epsilon: CC5 §9.0's note is that a
/// matte multiplies the *difference* a node made, so a sub-ULP leak outside
/// the matte is exactly the failure this gate exists to catch, and no
/// tolerance may excuse it.
fn containment_counts(actual: &[f32], baseline: &[f32], inside: &[bool]) -> ContainmentCounts {
    assert_eq!(actual.len(), baseline.len());
    assert_eq!(actual.len(), inside.len() * 4);
    let mut counts = ContainmentCounts {
        inside_pixels: 0,
        outside_pixels: 0,
        inside_changed_pixels: 0,
        outside_changed_pixels: 0,
        inside_changed_samples: 0,
        inside_samples: 0,
    };
    for (index, covered) in inside.iter().enumerate() {
        let actual = &actual[index * 4..index * 4 + 3];
        let baseline = &baseline[index * 4..index * 4 + 3];
        let changed = actual
            .iter()
            .zip(baseline)
            .filter(|(actual, baseline)| actual.to_bits() != baseline.to_bits())
            .count();
        if *covered {
            counts.inside_pixels += 1;
            counts.inside_samples += 3;
            counts.inside_changed_samples += changed as u64;
            if changed > 0 {
                counts.inside_changed_pixels += 1;
            }
        } else {
            counts.outside_pixels += 1;
            if changed > 0 {
                counts.outside_changed_pixels += 1;
            }
        }
    }
    counts
}

/// The CC5 §9.0 rule 7 two-sided vacuity gate.
fn assert_matte_containment(
    actual: &[f32],
    baseline: &[f32],
    inside: &[bool],
    label: &str,
) -> ContainmentCounts {
    let counts = containment_counts(actual, baseline, inside);
    assert_eq!(
        counts.outside_changed_pixels, 0,
        "case {label} changed {} of {} pixels OUTSIDE the matte; CC5 §9.0.7 allows exactly zero \
         and no tolerance may excuse one",
        counts.outside_changed_pixels, counts.outside_pixels
    );
    assert!(
        counts.inside_changed_samples * 10_000
            >= counts.inside_samples * MIN_CHANGED_LINEAR_BASIS_POINTS,
        "case {label} changed only {} of {} linear RGB samples INSIDE the matte; CC5 §9.0.7 \
         requires at least {MIN_CHANGED_LINEAR_BASIS_POINTS} basis points or the case proves \
         nothing",
        counts.inside_changed_samples,
        counts.inside_samples
    );
    counts
}

/// The same gate on monitor RGBA8 bytes: outside pixels must be byte-identical
/// and the alpha byte must never move, anywhere.
fn assert_monitor_containment(actual: &[u8], baseline: &[u8], inside: &[bool], label: &str) {
    assert_eq!(actual.len(), baseline.len());
    assert_eq!(actual.len(), inside.len() * 4);
    for (index, covered) in inside.iter().enumerate() {
        let actual = &actual[index * 4..index * 4 + 4];
        let baseline = &baseline[index * 4..index * 4 + 4];
        assert_eq!(
            actual[3], baseline[3],
            "case {label} moved the alpha byte at pixel {index}: CC5 §2.5.2 forbids every CC5 \
             code path from touching alpha"
        );
        if !covered {
            assert_eq!(
                &actual[..3],
                &baseline[..3],
                "case {label} changed monitor pixel {index}, which is outside the matte"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Evidence.
// ---------------------------------------------------------------------------

fn emit_cc5_evidence(
    fixture: &str,
    backend: &str,
    lane: &str,
    controls: Value,
    raster: (u32, u32),
    output_hash: String,
    metrics: Value,
) {
    assert!(
        CC5_EVIDENCE_FIXTURES.contains(&fixture),
        "every CC5 evidence payload must be declared in CC5_EVIDENCE_FIXTURES and in the manifest; {fixture} is not"
    );
    let provenance = backend_metadata(backend);
    let field = |key: &str| provenance.get(key).cloned().unwrap_or(Value::Null);
    let payload = json!({
        "contract": CC5_CONTRACT,
        "fixture": fixture,
        "lane": lane,
        "git_revision": git_revision(),
        "backend": backend,
        "backend_name": field("backend"),
        "adapter": field("adapter"),
        "software_fallback": field("software_fallback"),
        "gpu_claim": field("gpu_claim"),
        "backend_lane": field("lane"),
        "backend_metadata": provenance,
        "os": std::env::consts::OS,
        "arch": std::env::consts::ARCH,
        "raster": {"width": raster.0, "height": raster.1},
        "controls": controls,
        "output_hash_sha256": output_hash,
        "metrics": metrics,
    });
    println!("CC5_EVIDENCE {payload}");
    write_evidence_artefact(fixture, &payload);
}

// ---------------------------------------------------------------------------
// §9.1: the rasters.
// ---------------------------------------------------------------------------

/// CC5 §9.1. Both rasters exercise what they claim to, the CPU reference's uv
/// at `(x, y)` is the pixel centre, and the GPU's `@builtin(position)` maps to
/// the same pixel.
#[test]
fn cc5_rasters_cover_their_controls_and_land_on_pixel_centres() {
    let field = cc5_field_raster();
    assert_eq!(field.len(), CC5_RASTER_PIXELS);
    for (index, sample) in field.iter().enumerate() {
        for (channel, value) in sample.iter().enumerate() {
            assert!(
                (0.05..=0.95).contains(value),
                "field raster pixel {index} channel {channel} is {value}, outside [0.05, 0.95]"
            );
            assert_ne!(
                *value, 0.0,
                "CC5 §9.1 forbids an exact zero in the field raster: a gain node leaves it \
                 unchanged, which would make 'exactly 0 outside changed' pass for the wrong reason"
            );
        }
    }
    // Strictly varying in x, in y, and in both.
    for y in 0..CC5_RASTER_HEIGHT {
        for x in 1..CC5_RASTER_WIDTH {
            let previous = field[raster_index((x - 1, y))];
            let current = field[raster_index((x, y))];
            assert!(current[0] > previous[0], "red must strictly increase in x");
            assert!(current[2] > previous[2], "blue must strictly increase in x");
            assert_eq!(current[1], previous[1], "green varies only in y");
        }
    }
    for x in 0..CC5_RASTER_WIDTH {
        for y in 1..CC5_RASTER_HEIGHT {
            let previous = field[raster_index((x, y - 1))];
            let current = field[raster_index((x, y))];
            assert!(
                current[1] > previous[1],
                "green must strictly increase in y"
            );
            assert!(current[2] > previous[2], "blue must strictly increase in y");
            assert_eq!(current[0], previous[0], "red varies only in x");
        }
    }

    // --- the parity raster inherits CC3's value coverage ------------------
    let parity = cc5_parity_raster();
    assert_eq!(parity.len(), CC5_RASTER_PIXELS);
    let samples = cc3_parity_raster();
    for (index, sample) in parity.iter().enumerate() {
        let x = (index as u32 % CC5_RASTER_WIDTH) / CC5_PARITY_BLOCK_WIDTH;
        let y = (index as u32 / CC5_RASTER_WIDTH) / CC5_PARITY_BLOCK_HEIGHT;
        assert_eq!(
            *sample,
            samples[(y * CC5_PARITY_BLOCK_COLUMNS + x) as usize],
            "parity raster block ({x}, {y}) does not carry its CC3 §10.2 sample"
        );
    }
    let negatives = parity
        .iter()
        .filter(|sample| sample.iter().any(|value| *value < 0.0))
        .count();
    let in_unit = parity
        .iter()
        .filter(|sample| sample.iter().all(|value| (0.0..=1.0).contains(value)))
        .count();
    let over_range = parity
        .iter()
        .filter(|sample| sample.iter().any(|value| *value > 1.0))
        .count();
    assert!(negatives > 0 && in_unit > 0 && over_range > 0);
    // The parity raster varies in both axes, unlike CC3's single-axis bars.
    let first_row = &parity[..CC5_RASTER_WIDTH as usize];
    let first_column = (0..CC5_RASTER_HEIGHT)
        .map(|y| parity[raster_index((0, y))])
        .collect::<Vec<_>>();
    assert!(
        first_row.iter().any(|sample| *sample != first_row[0]),
        "the parity raster must vary along x"
    );
    assert!(
        first_column.iter().any(|sample| *sample != first_column[0]),
        "the parity raster must vary along y"
    );

    // --- the §9.2 window's edges cross block boundaries in both axes ------
    let matte = MatteSpec::window(WindowSpec::CENTRED);
    let covered = matte.covered_pixels(&parity);
    let mut inside_blocks = 0_u32;
    let mut outside_blocks = 0_u32;
    let mut split_columns = 0_u32;
    for block_y in 0..CC5_PARITY_BLOCK_ROWS {
        for block_x in 0..CC5_PARITY_BLOCK_COLUMNS {
            let mut any = false;
            let mut all = true;
            for y in 0..CC5_PARITY_BLOCK_HEIGHT {
                for x in 0..CC5_PARITY_BLOCK_WIDTH {
                    let pixel = (
                        block_x * CC5_PARITY_BLOCK_WIDTH + x,
                        block_y * CC5_PARITY_BLOCK_HEIGHT + y,
                    );
                    if covered[raster_index(pixel)] {
                        any = true;
                    } else {
                        all = false;
                    }
                }
            }
            if all {
                inside_blocks += 1;
            } else if !any {
                outside_blocks += 1;
            }
        }
    }
    for block_x in 0..CC5_PARITY_BLOCK_COLUMNS {
        let left = covered[raster_index((block_x * CC5_PARITY_BLOCK_WIDTH, 18))];
        let right = covered[raster_index((block_x * CC5_PARITY_BLOCK_WIDTH + 3, 18))];
        if left != right {
            split_columns += 1;
        }
    }
    // The window's x edges fall at block columns 4 and 12 and its y edges at
    // block rows 3 and 9 — interior in both axes, so both fully covered and
    // fully uncovered blocks exist and no block straddles an edge.
    assert_eq!(
        inside_blocks,
        8 * 6,
        "the window must fill 8 × 6 whole blocks"
    );
    assert_eq!(
        inside_blocks + outside_blocks,
        CC5_PARITY_BLOCK_COLUMNS * CC5_PARITY_BLOCK_ROWS,
        "no parity block may straddle the window edge, or a parity sample would be half graded"
    );
    assert_eq!(split_columns, 0);

    // --- the pixel-centre correspondence, on both implementations ---------
    let frame = frame_of(&field);
    for index in [0_usize, 1, 64, 1_234, CC5_RASTER_PIXELS - 1] {
        let actual = pixel_centre_uv(&frame, index);
        let expected = spec_pixel_centre_uv_f64(index);
        assert_eq!(f64::from(actual[0]), f64::from(expected[0] as f32));
        assert_eq!(f64::from(actual[1]), f64::from(expected[1] as f32));
    }
    // The GPU's `@builtin(position)` must land on the same centres: a
    // half-pixel offset would shift the hard-edged covered set by a column and
    // a row, so a byte-exact coverage comparison is the assertion.
    let gpu = fallback_gpu();
    let compositor = Compositor::new(gpu.context());
    let effects = [gain_wheels(1, 1_500, Some(&matte))];
    let coverage = gpu_coverage(&compositor, &frame, &effects, EffectId(1));
    let expected_coverage = covered
        .iter()
        .map(|covered| u8::from(*covered) * 255)
        .collect::<Vec<_>>();
    assert_eq!(
        coverage, expected_coverage,
        "the GPU coverage raster must be exactly the hand-derived covered set, which it is only \
         if `@builtin(position)` maps to the pixel centres the CPU reference uses"
    );

    emit_cc5_evidence(
        "cc5_rasters",
        gpu.backend(),
        gpu.lane.id(),
        json!({
            "field_raster": "r = 0.05 + 0.9x/63, g = 0.05 + 0.9y/35, b = 0.05 + 0.45(x/63 + y/35)",
            "parity_raster": "CC3 §10.2 192 samples as a 16 × 12 grid of 4 × 3 blocks",
            "window": "centred rect 2500/2500, feather 0",
        }),
        CC5_RESOLUTION,
        output_hash(&coverage),
        json!({
            "parity_samples_negative": negatives,
            "parity_samples_in_unit": in_unit,
            "parity_samples_over_range": over_range,
            "fully_covered_blocks": inside_blocks,
            "fully_uncovered_blocks": outside_blocks,
        }),
    );
}

// ---------------------------------------------------------------------------
// §9.2.1: affected-pixel containment, the central gate.
// ---------------------------------------------------------------------------

/// The hand-derived covered set of the §9.2.1 centred window: columns
/// `x ∈ 16..=47` crossed with rows `y ∈ 9..=26`.
///
/// Derived from `|u.x − 0.5| ≤ 0.25` and `|u.y − 0.5| ≤ 0.25` with
/// `u = ((x + 0.5)/64, (y + 0.5)/36)`: `x + 0.5 ∈ [16, 48]` and
/// `y + 0.5 ∈ [9, 27]`, so no pixel centre lies on either boundary.
fn hand_derived_centred_window() -> Vec<bool> {
    (0..CC5_RASTER_PIXELS)
        .map(|index| {
            let x = index as u32 % CC5_RASTER_WIDTH;
            let y = index as u32 / CC5_RASTER_WIDTH;
            (16..=47).contains(&x) && (9..=26).contains(&y)
        })
        .collect()
}

/// The smallest `|D − 1|` any pixel centre attains for `window`, in f64.
///
/// A margin is evidence, not decoration: it is what says the hand-derived
/// covered set is a property of the geometry rather than of f32 rounding.
fn smallest_boundary_margin(window: &WindowSpec) -> f64 {
    (0..CC5_RASTER_PIXELS)
        .map(|index| {
            (spec_window_distance_f64(
                window,
                spec_pixel_centre_uv_f64(index),
                SPEC_RASTER_ASPECT_F64,
            ) - 1.0)
                .abs()
        })
        .fold(f64::INFINITY, f64::min)
}

/// CC5 §9.2.1, the central gate. The window covers exactly the hand-derived
/// 576 pixels; every one of them changes; **no** pixel outside it changes, in
/// linear working values and in monitor RGBA8, on the CPU reference and on the
/// production GPU; alpha never moves; and `matte_invert` swaps the two sets.
#[test]
fn cc5_affected_pixel_containment_is_exact_on_cpu_and_gpu() {
    let raster = cc5_field_raster();
    let frame = frame_of(&raster);
    let matte = MatteSpec::window(WindowSpec::CENTRED);

    // --- the covered set is hand-derived, twice --------------------------
    let expected = hand_derived_centred_window();
    let covered = matte.covered_pixels(&raster);
    assert_eq!(
        covered, expected,
        "the independent §2.3 transcription must reproduce the hand-derived covered set"
    );
    let inside_count = covered.iter().filter(|covered| **covered).count();
    assert_eq!(inside_count, CENTRED_WINDOW_PIXELS);
    assert_eq!(
        CC5_RASTER_PIXELS - inside_count,
        CENTRED_WINDOW_OUTSIDE_PIXELS
    );
    assert_eq!(
        inside_count as u64 * 10_000 / CC5_RASTER_PIXELS as u64,
        CENTRED_WINDOW_BASIS_POINTS
    );
    let margin = smallest_boundary_margin(&WindowSpec::CENTRED);
    assert!(
        margin > 0.03,
        "no pixel centre may sit on the window boundary; smallest |D − 1| is {margin}"
    );

    let graded = gain_wheels(1, 1_500, Some(&matte));
    let unmatted = gain_wheels(1, 1_500, None);
    let inverted_matte = MatteSpec::window(WindowSpec::CENTRED).inverted();
    let inverted = gain_wheels(1, 1_500, Some(&inverted_matte));

    // --- CPU reference ----------------------------------------------------
    let baseline_linear = cpu_reference_linear(&frame, &[]);
    let baseline_monitor = cpu_reference_monitor(&frame, &[]);
    let cpu_linear = cpu_reference_linear(&frame, &cpu_nodes(std::slice::from_ref(&graded)));
    let cpu_monitor = cpu_reference_monitor(&frame, &cpu_nodes(std::slice::from_ref(&graded)));
    let cpu_counts = assert_matte_containment(&cpu_linear, &baseline_linear, &covered, "cpu");
    assert_eq!(
        cpu_counts.inside_changed_pixels, CENTRED_WINDOW_PIXELS,
        "every one of the 576 inside pixels must change: a master gain moves every non-zero \
         channel, and the §9.1 raster has none"
    );
    assert_monitor_containment(&cpu_monitor, &baseline_monitor, &covered, "cpu");

    // Alpha is byte-identical to the same stack with the matte removed.
    let unmatted_monitor =
        cpu_reference_monitor(&frame, &cpu_nodes(std::slice::from_ref(&unmatted)));
    for index in 0..CC5_RASTER_PIXELS {
        assert_eq!(
            cpu_monitor[index * 4 + 3],
            unmatted_monitor[index * 4 + 3],
            "the matte changed an alpha byte at pixel {index}; CC5 §2.5.2 forbids it"
        );
    }

    // --- the invert swaps the two sets exactly ---------------------------
    let complement = covered.iter().map(|covered| !covered).collect::<Vec<_>>();
    let cpu_inverted = cpu_reference_linear(&frame, &cpu_nodes(std::slice::from_ref(&inverted)));
    let inverted_counts =
        assert_matte_containment(&cpu_inverted, &baseline_linear, &complement, "cpu_inverted");
    assert_eq!(
        inverted_counts.inside_changed_pixels, CENTRED_WINDOW_OUTSIDE_PIXELS,
        "matte_invert = 1 must change exactly the 1728 pixels the un-inverted matte left alone"
    );
    assert_eq!(inverted_counts.outside_pixels, CENTRED_WINDOW_PIXELS);

    // --- the production GPU ----------------------------------------------
    let gpu = fallback_gpu();
    let compositor = Compositor::new(gpu.context());
    let gpu_baseline_linear = gpu_linear(&compositor, &frame, &[], None);
    let gpu_baseline_monitor = gpu_monitor(&compositor, &frame, &[], None);
    let gpu_matted_linear = gpu_linear(&compositor, &frame, std::slice::from_ref(&graded), None);
    let gpu_matted_monitor = gpu_monitor(&compositor, &frame, std::slice::from_ref(&graded), None);
    let gpu_counts =
        assert_matte_containment(&gpu_matted_linear, &gpu_baseline_linear, &covered, "gpu");
    assert_eq!(gpu_counts.inside_changed_pixels, CENTRED_WINDOW_PIXELS);
    assert_monitor_containment(&gpu_matted_monitor, &gpu_baseline_monitor, &covered, "gpu");
    let gpu_unmatted_monitor =
        gpu_monitor(&compositor, &frame, std::slice::from_ref(&unmatted), None);
    for index in 0..CC5_RASTER_PIXELS {
        assert_eq!(
            gpu_matted_monitor[index * 4 + 3],
            gpu_unmatted_monitor[index * 4 + 3],
            "the GPU matte changed an alpha byte at pixel {index}"
        );
    }
    let gpu_inverted_linear =
        gpu_linear(&compositor, &frame, std::slice::from_ref(&inverted), None);
    let gpu_inverted_counts = assert_matte_containment(
        &gpu_inverted_linear,
        &gpu_baseline_linear,
        &complement,
        "gpu_inverted",
    );
    assert_eq!(
        gpu_inverted_counts.inside_changed_pixels,
        CENTRED_WINDOW_OUTSIDE_PIXELS
    );

    // --- §2.5.5: −0.0 and a non-finite node output outside the matte ------
    let over_range = cc5_over_range_raster();
    let over_range_frame = frame_of(&over_range);
    let overflow_matte = MatteSpec::window(WindowSpec::CENTRED);
    let overflow = overflow_wheels(1, Some(&overflow_matte));
    let overflow_unmatted = overflow_wheels(1, None);
    let negative_zero = raster_index(NEGATIVE_ZERO_PIXEL);
    let over_range_index = raster_index(OVER_RANGE_PIXEL);
    let negative_index = raster_index(NEGATIVE_PIXEL);
    for index in [negative_zero, over_range_index, negative_index] {
        assert!(
            !covered[index],
            "the §2.5.5 samples must sit OUTSIDE the matte, or the clause is not exercised"
        );
    }
    // The premise, from the independent f32 transcription of CC3 §2.2: with
    // `slope = power = 16` the node output at the over-range sample really is
    // non-finite, so `x + (node(x) − x)·0.0` would be NaN rather than `x`.
    let overflow_output = spec_wheels_apply_f32(16.0, 0.0, 16.0, 4.0);
    assert!(
        !overflow_output.is_finite(),
        "the §9.2.1 over-range premise requires a non-finite node output; the transcription gives \
         {overflow_output}"
    );
    let over_range_baseline = cpu_reference_linear(&over_range_frame, &[]);
    let over_range_cpu = cpu_reference_linear(
        &over_range_frame,
        &cpu_nodes(std::slice::from_ref(&overflow)),
    );
    let over_range_counts = assert_matte_containment(
        &over_range_cpu,
        &over_range_baseline,
        &covered,
        "cpu_over_range",
    );
    assert_eq!(
        over_range_counts.inside_changed_pixels,
        CENTRED_WINDOW_PIXELS
    );
    assert_eq!(
        over_range_cpu[negative_zero * 4].to_bits(),
        (-0.0_f32).to_bits(),
        "CC5 §2.5.5: a −0.0 outside the matte must survive as −0.0, not as +0.0"
    );
    assert!(
        over_range_cpu[over_range_index * 4].is_finite(),
        "the over-range sample outside the matte must keep its finite input value"
    );
    assert!(
        over_range_cpu[negative_index * 4] < 0.0,
        "a genuine negative outside the matte must stay negative"
    );

    // The GPU half: `−0.0` cannot be asserted, because the working-surface
    // upload normalises it to `+0.0` before the node stack runs (the no-node
    // baseline already carries `+0.0`), so the gate is bit-equality against
    // that baseline plus a genuine negative surviving.
    let gpu_over_range_baseline = gpu_linear(&compositor, &over_range_frame, &[], None);
    assert_eq!(
        gpu_over_range_baseline[negative_zero * 4].to_bits(),
        0.0_f32.to_bits(),
        "measured: the GPU upload/sample path normalises −0.0 to +0.0 before the node stack"
    );
    let gpu_over_range = gpu_linear(
        &compositor,
        &over_range_frame,
        std::slice::from_ref(&overflow),
        None,
    );
    assert_matte_containment(
        &gpu_over_range,
        &gpu_over_range_baseline,
        &covered,
        "gpu_over_range",
    );
    assert!(
        gpu_over_range[negative_index * 4] < 0.0,
        "a genuine negative outside the matte must survive the GPU node stack"
    );
    // And the un-matted stack really does reach the non-finite state on the
    // GPU too, so the bit-identical outside pixel is a preserved input rather
    // than a coincidence.
    let gpu_over_range_unmatted = gpu_linear(
        &compositor,
        &over_range_frame,
        std::slice::from_ref(&overflow_unmatted),
        None,
    );
    assert!(
        !gpu_over_range_unmatted[over_range_index * 4].is_finite(),
        "without the matte the GPU node output at the over-range sample must be non-finite"
    );

    emit_cc5_evidence(
        "cc5_containment",
        gpu.backend(),
        gpu.lane.id(),
        json!({
            "node": "color_wheels gain_master_thousandths = 1500",
            "window": "rect, centre (5000, 5000), half extents 2500/2500, feather 0",
            "over_range_node": "color_wheels gain/gamma 4000 on every channel (slope = power = 16)",
        }),
        CC5_RESOLUTION,
        output_hash(&gpu_matted_monitor),
        json!({
            "covered_pixels": inside_count,
            "coverage_basis_points": CENTRED_WINDOW_BASIS_POINTS,
            "smallest_boundary_margin": margin,
            "cpu": cpu_counts.as_json(),
            "gpu": gpu_counts.as_json(),
            "cpu_inverted": inverted_counts.as_json(),
            "gpu_inverted": gpu_inverted_counts.as_json(),
            "over_range": over_range_counts.as_json(),
            "over_range_node_output": format!("{overflow_output}"),
        }),
    );
}

// ---------------------------------------------------------------------------
// §9.2.2: window geometry anchors.
// ---------------------------------------------------------------------------

/// Compare a GPU coverage raster against the independent transcription's
/// `round(255 · m)`.
///
/// The failure message names the first divergent pixel and the divergence
/// histogram rather than printing two 2304-byte vectors, so a regression is
/// readable.
fn assert_coverage_matches(actual: &[u8], expected: &[f64], hard_edged: bool, label: &str) {
    assert_eq!(actual.len(), expected.len());
    let tolerance = if hard_edged {
        0
    } else {
        MATTE_PROOF_FEATHERED_CODE_TOLERANCE
    };
    let mut worst = (0_usize, 0_u8);
    let mut divergent = 0_usize;
    for (index, (actual, expected)) in actual.iter().zip(expected).enumerate() {
        let expected = (expected.clamp(0.0, 1.0) * 255.0).round() as u8;
        let difference = actual.abs_diff(expected);
        if difference > 0 {
            divergent += 1;
        }
        if difference > worst.1 {
            worst = (index, difference);
        }
    }
    assert!(
        worst.1 <= tolerance,
        "GPU coverage for {label} diverges from round(255·m) by {} codes at pixel {} ({divergent} \
         pixels differ at all); CC5 §9.2.9 allows {tolerance}",
        worst.1,
        worst.0
    );
}

/// Assert one window's hand-derived covered set on the CPU reference and on
/// the production GPU, and return that set.
///
/// The GPU half compares the coverage raster byte for byte against
/// `255 · [m > 0]`, which for an unfeathered window is the whole coverage
/// function, not a sample of it.
fn assert_window_case(
    compositor: &Compositor,
    frame: &WorkingFrame,
    raster: &[[f32; 3]],
    window: WindowSpec,
    expected_pixels: usize,
    label: &str,
) -> Vec<bool> {
    assert_matte_case(
        compositor,
        frame,
        raster,
        &MatteSpec::window(window),
        expected_pixels,
        label,
    )
}

/// [`assert_window_case`] for a whole matte: any window list, combine token,
/// invert, mix, and qualifier.
fn assert_matte_case(
    compositor: &Compositor,
    frame: &WorkingFrame,
    raster: &[[f32; 3]],
    matte: &MatteSpec,
    expected_pixels: usize,
    label: &str,
) -> Vec<bool> {
    let covered = matte.covered_pixels(raster);
    let count = covered.iter().filter(|covered| **covered).count();
    assert_eq!(
        count, expected_pixels,
        "window {label} covers {count} pixels; the contract derives {expected_pixels} by hand"
    );
    let graded = gain_wheels(1, 1_500, Some(matte));
    let baseline = cpu_reference_linear(frame, &[]);
    let cpu = cpu_reference_linear(frame, &cpu_nodes(std::slice::from_ref(&graded)));
    let counts = assert_matte_containment(&cpu, &baseline, &covered, label);
    assert_eq!(counts.inside_changed_pixels, expected_pixels);
    let coverage = gpu_coverage(
        compositor,
        frame,
        std::slice::from_ref(&graded),
        EffectId(1),
    );
    assert_coverage_matches(
        &coverage,
        &matte.coverage_values(raster),
        matte.is_hard_edged(),
        label,
    );
    let gpu_baseline = gpu_linear(compositor, frame, &[], None);
    let gpu = gpu_linear(compositor, frame, std::slice::from_ref(&graded), None);
    assert_matte_containment(&gpu, &gpu_baseline, &covered, &format!("{label}_gpu"));
    covered
}

/// The pixel-space bounding box `(x_min, x_max, y_min, y_max)` of a covered
/// set, or `None` when it is empty.
fn covered_bounding_box(covered: &[bool]) -> Option<(u32, u32, u32, u32)> {
    let mut box_: Option<(u32, u32, u32, u32)> = None;
    for (index, is_covered) in covered.iter().enumerate() {
        if !is_covered {
            continue;
        }
        let x = index as u32 % CC5_RASTER_WIDTH;
        let y = index as u32 / CC5_RASTER_WIDTH;
        box_ = Some(match box_ {
            None => (x, x, y, y),
            Some((left, right, top, bottom)) => {
                (left.min(x), right.max(x), top.min(y), bottom.max(y))
            }
        });
    }
    box_
}

/// CC5 §9.2.2. Every window anchor is hand-derived above and asserted on the
/// CPU reference and on the GPU, including the rotation case that is the
/// **aspect gate**.
#[test]
fn cc5_window_geometry_anchors_are_hand_derived_on_cpu_and_gpu() {
    let raster = cc5_field_raster();
    let frame = frame_of(&raster);
    let gpu = fallback_gpu();
    let compositor = Compositor::new(gpu.context());

    // --- the four anchors -------------------------------------------------
    let centred = assert_window_case(
        &compositor,
        &frame,
        &raster,
        WindowSpec::CENTRED,
        576,
        "centred_rect_2500",
    );
    assert_eq!(covered_bounding_box(&centred), Some((16, 47, 9, 26)));

    let square = assert_window_case(
        &compositor,
        &frame,
        &raster,
        WindowSpec::PIXEL_SQUARE,
        196,
        "pixel_square_rect_rotation_0",
    );
    // `hw·a = hh = 0.2` is 7.2 px each way, and the pixel offsets from the
    // centre are half-integers, so `|d| ≤ 7.2` selects 14 columns × 14 rows.
    assert_eq!(covered_bounding_box(&square), Some((25, 38, 11, 24)));

    let rotated = assert_window_case(
        &compositor,
        &frame,
        &raster,
        WindowSpec::PIXEL_SQUARE.with_rotation(4_500),
        220,
        "pixel_square_rect_rotation_4500",
    );
    // The aspect gate: at 45° the covered set is symmetric under
    // `(dx, dy) → (dy, dx)` only if the aspect correction is applied, because
    // without it the rotation shears the window into a parallelogram.
    // `dx = x + 0.5 − 32`, `dy = y + 0.5 − 18`, so the partner of `(x, y)` is
    // `(y + 14, x − 14)`.
    let mut transposed_pairs = 0_usize;
    for (index, is_covered) in rotated.iter().enumerate() {
        let x = index as u32 % CC5_RASTER_WIDTH;
        let y = index as u32 / CC5_RASTER_WIDTH;
        let partner = y
            .checked_add(14)
            .zip(x.checked_sub(14))
            .filter(|(partner_x, partner_y)| {
                *partner_x < CC5_RASTER_WIDTH && *partner_y < CC5_RASTER_HEIGHT
            });
        let Some(partner) = partner else {
            assert!(
                !is_covered,
                "covered pixel ({x}, {y}) has no transposed partner on the raster, so the \
                 symmetry could not be checked there"
            );
            continue;
        };
        assert_eq!(
            *is_covered,
            rotated[raster_index(partner)],
            "the 45° covered set is not symmetric under (dx, dy) → (dy, dx) at ({x}, {y}); \
             the aspect correction of CC5 §2.3 is not being applied"
        );
        if *is_covered {
            transposed_pairs += 1;
        }
    }
    assert_eq!(
        transposed_pairs, 220,
        "every covered pixel must have been checked against its transposed partner"
    );
    // `|dx ± dy| ≤ 7.2√2 = 10.18234` with `dx ± dy` integers of odd sum, which
    // counts 11·10 + 10·11 = 220 pixels.
    let mut hand_rotated = 0_usize;
    for index in 0..CC5_RASTER_PIXELS {
        let dx = (index as i64 % i64::from(CC5_RASTER_WIDTH)) * 2 + 1 - 64;
        let dy = (index as i64 / i64::from(CC5_RASTER_WIDTH)) * 2 + 1 - 36;
        let s = i64::midpoint(dx, dy);
        let t = i64::midpoint(dy, -dx);
        if (dx + dy) % 2 == 0 && s.abs() <= 10 && t.abs() <= 10 && (s + t) % 2 != 0 {
            hand_rotated += 1;
        }
    }
    assert_eq!(hand_rotated, 220);

    let ellipse = assert_window_case(
        &compositor,
        &frame,
        &raster,
        WindowSpec::PIXEL_SQUARE.with_shape(SHAPE_ELLIPSE),
        164,
        "pixel_square_ellipse_rotation_0",
    );
    assert_eq!(
        covered_bounding_box(&ellipse),
        Some((25, 38, 11, 24)),
        "the ellipse's bounding box is 14 × 14 pixels"
    );
    // `dx² + dy² ≤ 51.84`, counted per quadrant as 7, 7, 7, 6, 6, 5, 3 = 41.
    let mut quadrant = [0_usize; 7];
    for index in 0..CC5_RASTER_PIXELS {
        let dx = index as f64 % f64::from(CC5_RASTER_WIDTH) + 0.5 - 32.0;
        let dy = (index / CC5_RASTER_WIDTH as usize) as f64 + 0.5 - 18.0;
        if dx > 0.0 && dy > 0.0 && dx * dx + dy * dy <= 51.84 {
            quadrant[(dy - 0.5) as usize] += 1;
        }
    }
    assert_eq!(quadrant, [7, 7, 7, 6, 6, 5, 3]);
    assert_eq!(quadrant.iter().sum::<usize>() * 4, 164);
    // The margins: `(2i+1)² + (2j+1)² = 207.36` has no integer solution, so no
    // pixel centre is on the boundary.
    let mut interior_margin = f64::INFINITY;
    let mut exterior_margin = f64::INFINITY;
    for (index, covered) in ellipse.iter().enumerate() {
        let dx = index as f64 % f64::from(CC5_RASTER_WIDTH) + 0.5 - 32.0;
        let dy = (index / CC5_RASTER_WIDTH as usize) as f64 + 0.5 - 18.0;
        let radius = dx * dx + dy * dy;
        if *covered {
            interior_margin = interior_margin.min(51.84 - radius);
        } else {
            exterior_margin = exterior_margin.min(radius - 51.84);
        }
    }
    assert!(
        (interior_margin - 1.34).abs() < 1.0e-9,
        "smallest interior margin is {interior_margin} px², the contract derives 1.34"
    );
    assert!(
        (exterior_margin - 2.66).abs() < 1.0e-9,
        "smallest exterior margin is {exterior_margin} px², the contract derives 2.66"
    );

    // --- the ellipse is circular in pixels -------------------------------
    let hw = f64::from(WindowSpec::PIXEL_SQUARE.hw as f32) / 10_000.0;
    let hh = f64::from(WindowSpec::PIXEL_SQUARE.hh as f32) / 10_000.0;
    let product = hw * SPEC_RASTER_ASPECT_F64;
    let ulp = f64::EPSILON * hh;
    assert!(
        (product - hh).abs() <= 4.0 * ulp,
        "in f64 hw·a = {product} against hh = {hh}: exact equality is not claimed, four ULP is"
    );
    for aspect in [
        CC5_RASTER_WIDTH as f32 / CC5_RASTER_HEIGHT as f32,
        16.0 / 9.0,
        1920.0 / 1080.0,
    ] {
        let hw = WindowSpec::PIXEL_SQUARE.hw as f32 / 10_000.0;
        let hh = WindowSpec::PIXEL_SQUARE.hh as f32 / 10_000.0;
        assert_eq!(
            (hw * aspect).to_bits(),
            hh.to_bits(),
            "the shader-consumed constants must be the same f32 bit pattern at aspect {aspect}"
        );
    }
    // And the production resolver agrees on those constants, which is what the
    // shader and the reference actually consume.
    let window = MatteWindow::from_params(
        MatteParams::from_effect(&gain_wheels(
            1,
            1_500,
            Some(&MatteSpec::window(WindowSpec::PIXEL_SQUARE)),
        ))
        .window(0)
        .expect("window 0 resolves"),
    );
    assert_eq!(window.half_extents(), [0.1125, 0.2]);
    assert_eq!(window.rotation_cos_sin(), [1.0, 0.0]);

    emit_cc5_evidence(
        "cc5_window_geometry",
        gpu.backend(),
        gpu.lane.id(),
        json!({
            "centred_rect": "2500/2500",
            "pixel_square": "half_width 1125, half_height 2000 (hw·a = hh = 0.2, 7.2 px)",
            "rotations": [0, 4500],
            "shapes": ["rect", "ellipse"],
        }),
        CC5_RESOLUTION,
        json_hash(&json!({
            "centred": centred.iter().filter(|covered| **covered).count(),
            "square": square.iter().filter(|covered| **covered).count(),
            "rotated": rotated.iter().filter(|covered| **covered).count(),
            "ellipse": ellipse.iter().filter(|covered| **covered).count(),
        })),
        json!({
            "centred_rect_pixels": 576,
            "pixel_square_rect_pixels": 196,
            "rotated_rect_pixels": 220,
            "ellipse_pixels": 164,
            "ellipse_bounding_box": [25, 38, 11, 24],
            "ellipse_interior_margin_pixels_squared": interior_margin,
            "ellipse_exterior_margin_pixels_squared": exterior_margin,
            "f64_hw_times_aspect": product,
            "f64_hh": hh,
        }),
    );
}

// ---------------------------------------------------------------------------
// §9.0.2: every control at minimum, maximum, and an interior value has a
// numeric expected value.
//
// Two legs, because the rule has two halves:
//
// 1. the **sweep** puts each of the 47 `matte_*` controls at its descriptor
//    minimum, an interior value, and its descriptor maximum, and compares the
//    production `Matte::coverage` against `spec_coverage_f64` — the
//    independent f64 transcription of §2.3/§2.4/§2.5 — at nine probe pixels.
//    Each control is *isolated*: the windows that are not under test are
//    full-frame (weight `1` everywhere) and combined by intersection, so a
//    `min` cannot mask the window being swept, and the qualifier legs are
//    swept with no geometric restriction at all;
// 2. the **anchors** state a hand-derived literal for the non-trivial bounds,
//    with the derivation written out beside each one.
//
// Neither leg's expected value comes from the production code (§9.0 rule 1).
// ---------------------------------------------------------------------------

/// The nine probe pixels the §9.0.2 sweep compares at: the four corners, the
/// four pixels straddling the raster centre, and one interior pixel that is
/// not on any symmetry axis.
const CONTROL_PROBE_PIXELS: [(u32, u32); 9] = [
    (0, 0),
    (63, 0),
    (0, 35),
    (63, 35),
    (31, 17),
    (32, 17),
    (31, 18),
    (32, 18),
    (45, 7),
];

/// A window whose weight is exactly `1` at every pixel of the §9.1 raster.
///
/// `hw = hh = 1.0`, centred, so `D = max(|Δx|, |Δy|) ≤ 0.5 < 1` for every
/// `u ∈ (0, 1)²`. It is the identity of the intersection combine, which is
/// what lets the sweep isolate one window index at a time.
const FULL_FRAME_WINDOW: WindowSpec = WindowSpec {
    shape: SHAPE_RECT,
    cx: 5_000,
    cy: 5_000,
    hw: 10_000,
    hh: 10_000,
    rotation: 0,
    feather: 0,
    invert: 0,
};

/// The window the sweep mutates: off centre, rotated, and feathered, so every
/// one of the eight window controls has something to change.
const SWEPT_WINDOW: WindowSpec = WindowSpec {
    shape: SHAPE_RECT,
    cx: 5_500,
    cy: 4_500,
    hw: 2_500,
    hh: 3_000,
    rotation: 1_200,
    feather: 1_000,
    invert: 0,
};

/// The qualifier the sweep mutates: every leg engaged, no leg saturated.
const SWEPT_QUALIFIER: QualifierSpec = QualifierSpec {
    hue_center: 12_000,
    hue_width: 6_000,
    hue_softness: 2_000,
    sat_low: 1_000,
    sat_high: 9_000,
    sat_softness: 500,
    luma_low: 500,
    luma_high: 9_500,
    luma_softness: 800,
};

/// The base for the six matte-level controls: four distinct windows so
/// `matte_window_count` and `matte_combine_token` have work to do, plus an
/// enabled qualifier so `matte_window_count = 0` is still an *active* matte.
fn matte_level_base() -> MatteSpec {
    MatteSpec::window(WindowSpec::CENTRED)
        .with_windows(
            vec![
                WindowSpec::CENTRED,
                WindowSpec::CENTRED.with_centre(7_000, 5_000),
                WindowSpec::CENTRED
                    .with_centre(5_000, 3_000)
                    .with_feather(1_200),
                WindowSpec::CENTRED
                    .with_centre(3_000, 7_000)
                    .with_shape(SHAPE_ELLIPSE),
            ],
            COMBINE_UNION,
        )
        .with_qualifier(SWEPT_QUALIFIER)
}

/// One swept control's resolved matte, or `None` when that value resolves the
/// matte inactive (`matte_enabled = 0`, CC5 §2.6 rule 1).
fn control_sweep_spec(name: &str, value: i64) -> Option<MatteSpec> {
    // `matte_window{j}_*`, isolated: windows `0..j` are full-frame and the
    // combine is `min`, so the resolved coverage is exactly window `j`.
    if let Some((index, suffix)) = swept_window_control(name) {
        let mut window = SWEPT_WINDOW;
        match suffix {
            "shape_token" => window.shape = value,
            "center_x_basis_points" => window.cx = value,
            "center_y_basis_points" => window.cy = value,
            "half_width_basis_points" => window.hw = value,
            "half_height_basis_points" => window.hh = value,
            "rotation_centidegrees" => window.rotation = value,
            "feather_basis_points" => window.feather = value,
            "invert" => window.invert = value,
            other => panic!("CC5 §2.2 gained an unhandled window control {other}"),
        }
        let mut windows = vec![FULL_FRAME_WINDOW; index];
        windows.push(window);
        return Some(MatteSpec::window(window).with_windows(windows, COMBINE_INTERSECTION));
    }

    // The nine qualifier scalars, isolated: one full-frame window so the
    // matte stays active when the qualifier itself is switched off, and no
    // other geometry, so the coverage is exactly the qualifier weight.
    let mut qualifier = SWEPT_QUALIFIER;
    let qualifier_only = |qualifier: Option<QualifierSpec>| {
        let mut spec = MatteSpec::window(FULL_FRAME_WINDOW);
        spec.qualifier = qualifier;
        Some(spec)
    };
    match name {
        "matte_qualifier_enabled" => {
            return qualifier_only((value >= 1).then_some(qualifier));
        }
        "matte_hue_center_centidegrees" => {
            qualifier.hue_center = value;
            return qualifier_only(Some(qualifier));
        }
        "matte_hue_width_centidegrees" => {
            qualifier.hue_width = value;
            return qualifier_only(Some(qualifier));
        }
        "matte_hue_softness_centidegrees" => {
            qualifier.hue_softness = value;
            return qualifier_only(Some(qualifier));
        }
        "matte_saturation_low_basis_points" => {
            qualifier.sat_low = value;
            return qualifier_only(Some(qualifier));
        }
        "matte_saturation_high_basis_points" => {
            qualifier.sat_high = value;
            return qualifier_only(Some(qualifier));
        }
        "matte_saturation_softness_basis_points" => {
            qualifier.sat_softness = value;
            return qualifier_only(Some(qualifier));
        }
        "matte_luma_low_basis_points" => {
            qualifier.luma_low = value;
            return qualifier_only(Some(qualifier));
        }
        "matte_luma_high_basis_points" => {
            qualifier.luma_high = value;
            return qualifier_only(Some(qualifier));
        }
        "matte_luma_softness_basis_points" => {
            qualifier.luma_softness = value;
            return qualifier_only(Some(qualifier));
        }
        _ => {}
    }

    // The five matte-level controls.
    let mut spec = matte_level_base();
    match name {
        // `matte_enabled = 0` is the one value that makes the whole matte
        // inactive: the node must then be byte-identical to its CC4 self, so
        // there is no coverage function to compare (CC5 §2.6 rule 1).
        "matte_enabled" => {
            if value == 0 {
                return None;
            }
        }
        "matte_window_count" => {
            spec.windows
                .truncate(usize::try_from(value).expect("a non-negative window count"));
        }
        "matte_combine_token" => spec.combine = value,
        "matte_invert" => spec.invert = value,
        "matte_mix_basis_points" => spec.mix = value,
        other => panic!("CC5 §2.2 gained an unhandled matte control {other}"),
    }
    Some(spec)
}

/// Split `matte_window{j}_{suffix}` into its index and suffix.
///
/// `matte_window_count` shares the prefix and is deliberately **not** a window
/// control, so the index must parse as a number for the name to be one.
fn swept_window_control(name: &str) -> Option<(usize, &str)> {
    let (index, suffix) = name.strip_prefix("matte_window")?.split_once('_')?;
    Some((index.parse::<usize>().ok()?, suffix))
}

/// The production coverage of one spec, resolved through the real descriptor
/// and `MatteParams::from_effect`, or `None` for an inactive matte.
fn production_matte(spec: &MatteSpec) -> Option<Matte> {
    Matte::from_params(&MatteParams::from_effect(&gain_wheels(
        1,
        1_500,
        Some(spec),
    )))
}

/// The exactness the §9.0.2 sweep holds the production f32 evaluation to
/// against the f64 transcription.
///
/// Both sides evaluate the same written formulas; the only difference is the
/// working precision and the f32-rounded `(cosT, sinT)` pair both consume, so
/// this is an f32 round-off budget, not a modelling tolerance. Measured worst
/// divergence over all 47 controls × 3 bounds × 9 probes is recorded in the
/// evidence payload.
const CONTROL_SWEEP_TOLERANCE: f64 = 4.0e-6;

/// CC5 §9.0.2. Every one of the 47 `matte_*` controls, at its descriptor
/// minimum, an interior value, and its descriptor maximum, evaluated by the
/// production `Matte::coverage` and by the independent f64 transcription, at
/// nine probe pixels — plus hand-derived literal anchors for the non-trivial
/// bounds and two GPU coverage rasters.
#[test]
fn cc5_every_matte_control_bound_matches_a_hand_derived_expected_value() {
    let raster = cc5_field_raster();
    let frame = frame_of(&raster);
    let aspect = raster_aspect(&frame);
    let descriptor = effect_descriptor("color_wheels").expect("the descriptor exists");
    let names = matte_parameter_names();
    assert_eq!(names.len(), MATTE_PARAMETER_COUNT);
    assert_eq!(MATTE_PARAMETER_COUNT, 47);

    // --- leg 1: the sweep -------------------------------------------------
    let mut worst_divergence = 0.0_f64;
    let mut worst_label = String::new();
    let mut swept_values = 0_usize;
    let mut inactive_bounds = Vec::new();
    for name in names {
        let parameter = descriptor
            .parameter(name)
            .unwrap_or_else(|| panic!("color_wheels must carry {name}"));
        let interior = i64::midpoint(parameter.min, parameter.max);
        let mut bounds = vec![parameter.min, interior, parameter.max];
        bounds.dedup();
        for value in bounds {
            swept_values += 1;
            let label = format!("{name}={value}");
            let Some(spec) = control_sweep_spec(name, value) else {
                // The only inactive bound: the master switch off. Assert the
                // production resolution agrees, because §2.6 rule 1 is what
                // makes a pre-CC5 project render bit-identically.
                let mut disabled = gain_wheels(1, 1_500, Some(&matte_level_base()));
                disabled.parameters.insert(
                    "matte_enabled".to_owned(),
                    kinewright_core::ParamValue::Integer(0),
                );
                let params = MatteParams::from_effect(&disabled);
                assert!(
                    params.is_inactive_matte(),
                    "{label}: the master switch off must resolve the matte inactive"
                );
                assert!(
                    Matte::from_params(&params).is_none(),
                    "{label}: an inactive matte must not resolve to a coverage function"
                );
                inactive_bounds.push(label);
                continue;
            };
            let resolved = production_matte(&spec)
                .unwrap_or_else(|| panic!("{label}: the swept matte must stay active"));
            for (x, y) in CONTROL_PROBE_PIXELS {
                let index = (y * CC5_RASTER_WIDTH + x) as usize;
                let rgb = raster[index];
                let uv = pixel_centre_uv(&frame, index);
                let expected = spec_coverage_f64(
                    &spec,
                    spec_pixel_centre_uv_f64(index),
                    SPEC_RASTER_ASPECT_F64,
                    rgb,
                );
                let actual = f64::from(resolved.coverage(uv, aspect, rgb));
                let divergence = (actual - expected).abs();
                if divergence > worst_divergence {
                    worst_divergence = divergence;
                    worst_label = format!("{label} at ({x}, {y})");
                }
                assert!(
                    divergence <= CONTROL_SWEEP_TOLERANCE,
                    "{label} at pixel ({x}, {y}): the production coverage is {actual} and the \
                     independent §2 transcription derives {expected}"
                );
                assert!(
                    (0.0..=1.0).contains(&actual),
                    "{label} at pixel ({x}, {y}): coverage {actual} left 0..=1"
                );
            }
        }
    }
    assert_eq!(
        inactive_bounds,
        vec!["matte_enabled=0".to_owned()],
        "exactly one control bound may resolve the matte inactive"
    );
    // 47 controls × 3 bounds is 141, less the 12 two-valued tokens whose
    // interior `(min + max) / 2` collapses onto their minimum: `matte_enabled`,
    // `matte_combine_token`, `matte_invert`, `matte_qualifier_enabled`, and
    // `shape_token` plus `invert` on each of the four windows. 141 − 12 = 129.
    // Asserted rather than described, so a widened bound is visible here.
    assert_eq!(swept_values, 129);

    // --- leg 2: hand-derived literal anchors ------------------------------
    //
    // Every expectation below is derived from §2.3/§2.4 by hand, in the
    // comment beside it, and is a literal rather than a call.

    // (a) `center_x = -10000`, the minimum. The window spans
    //     `u.x ∈ [cx - hw, cx + hw] = [-2.0, 0.0]` at the maximum half-width,
    //     so its right edge is *exactly* the frame's left edge. Every pixel
    //     centre is `(x + 0.5)/64 > 0`, so `|n.x| = |u.x + 1.0| / 1.0 > 1` and
    //     `D > 1`: coverage is exactly 0 at all 2304 pixels.
    let off_frame_left = WindowSpec {
        cx: -10_000,
        hw: 10_000,
        hh: 10_000,
        ..WindowSpec::CENTRED
    };
    // (b) `center_x = 20000`, the maximum. Mirror image: the window spans
    //     `u.x ∈ [1.0, 3.0]`, its left edge is exactly the frame's right edge,
    //     and `|n.x| = |u.x - 2.0| = 2.0 - u.x > 1` for every `u.x < 1`.
    let off_frame_right = WindowSpec {
        cx: 20_000,
        hw: 10_000,
        hh: 10_000,
        ..WindowSpec::CENTRED
    };
    // (c) `half_width = 1`, the minimum: `hw = 0.0001`, i.e. 0.0064 px wide.
    //     Pixel centres in x are `(2x + 1)/128`, so the closest approach to
    //     `cx = 0.5` is `|63/128 - 64/128| = 1/128 = 0.0078125`, giving
    //     `|n.x| = 0.0078125 / 0.0001 = 78.125 > 1`. Coverage is exactly 0.
    let hair_width = WindowSpec {
        hw: 1,
        ..WindowSpec::CENTRED
    };
    // (d) `half_height = 1`, the minimum: pixel centres in y are
    //     `(2y + 1)/72`, closest approach to `cy = 0.5` is
    //     `|35/72 - 36/72| = 1/72 = 0.0138888…`, so
    //     `|n.y| = 138.888… > 1`. Coverage is exactly 0.
    let hair_height = WindowSpec {
        hh: 1,
        ..WindowSpec::CENTRED
    };
    // (e) `half_width = half_height = 10000`, the maximum: `hw = hh = 1.0`,
    //     so `D = max(|Δx|, |Δy|) ≤ 0.5 < 1` everywhere. Coverage is exactly
    //     1 at all 2304 pixels.
    for (label, window, expected, expected_covered) in [
        ("center_x_min_off_frame", off_frame_left, 0.0_f64, 0_usize),
        ("center_x_max_off_frame", off_frame_right, 0.0, 0),
        ("half_width_min", hair_width, 0.0, 0),
        ("half_height_min", hair_height, 0.0, 0),
        (
            "half_extents_max",
            FULL_FRAME_WINDOW,
            1.0,
            CC5_RASTER_PIXELS,
        ),
    ] {
        let spec = MatteSpec::window(window);
        let resolved = production_matte(&spec).expect("an active matte");
        let mut covered = 0_usize;
        for (index, rgb) in raster.iter().enumerate() {
            let actual = resolved.coverage(pixel_centre_uv(&frame, index), aspect, *rgb);
            assert_eq!(
                actual.to_bits(),
                (expected as f32).to_bits(),
                "{label}: pixel {index} has coverage {actual}, and the hand derivation gives \
                 {expected}"
            );
            if actual > 0.0 {
                covered += 1;
            }
        }
        assert_eq!(covered, expected_covered, "{label} covered count");
        // And the independent transcription derives the same thing, so the
        // literal is not merely what this build happens to compute.
        assert_eq!(
            spec.covered_pixels(&raster)
                .iter()
                .filter(|covered| **covered)
                .count(),
            expected_covered,
            "{label}: the §2.3 transcription disagrees with the hand derivation"
        );
    }
    // The closest-approach numbers the (c) and (d) derivations rest on.
    assert_eq!(
        (0.5_f64 - 63.0 / 128.0) * 10_000.0,
        78.125,
        "the closest x pixel centre is 78.125 basis points from the raster centre"
    );
    assert!(
        ((0.5_f64 - 35.0 / 72.0) * 10_000.0 - 138.888_888_888_888_9).abs() < 1e-9,
        "the closest y pixel centre is 138.889 basis points from the raster centre"
    );

    // (f) `rotation = ±18000`, the two bounds: exactly ±180°. A rect and an
    //     ellipse are both symmetric under a half turn — `d → −d` leaves
    //     `max(|n.x|, |n.y|)` and `n.x² + n.y²` unchanged — which is the
    //     contract's stated reason the range stops at ±180. So the covered
    //     set at ±18000 must be *identical* to the set at 0, i.e. the §9.2.2
    //     pixel-square anchor of 196 pixels.
    let upright = MatteSpec::window(WindowSpec::PIXEL_SQUARE).covered_pixels(&raster);
    assert_eq!(upright.iter().filter(|covered| **covered).count(), 196);
    for rotation in [18_000_i64, -18_000] {
        let spec = MatteSpec::window(WindowSpec::PIXEL_SQUARE.with_rotation(rotation));
        assert_eq!(
            spec.covered_pixels(&raster),
            upright,
            "rotation {rotation} centidegrees is a half turn and must not move the covered set"
        );
        let resolved = production_matte(&spec).expect("an active matte");
        for index in 0..CC5_RASTER_PIXELS {
            let actual = resolved.coverage(pixel_centre_uv(&frame, index), aspect, raster[index]);
            assert_eq!(
                actual > 0.0,
                upright[index],
                "rotation {rotation}: pixel {index} disagrees with the upright window"
            );
        }
    }

    // (g) `feather = 10000`, the maximum: `f = 1.0`, so
    //     `w = 1 - smoothstep(0, 2, D)` and the band spans `D ∈ [0, 2]`.
    //     On the centred 2500/2500 window, `D = max(|Δx|, |Δy|) / 0.25 < 2`
    //     for every `u ∈ (0, 1)²`, so every pixel is inside the band.
    //     At pixel (0, 0): `u = (0.5/64, 0.5/36)`, `Δx = -0.4921875`,
    //     `Δy = -0.4861111…`, so `n = (-1.96875, -1.9444…)` and
    //     `D = 1.96875` exactly (dyadic). Then `t = D/2 = 63/64`,
    //     `t² = 3969/4096`, `3 - 2t = 33/32`, and
    //     `smoothstep = 3969·33 / (4096·32) = 130977/131072`, so
    //     `w = 95/131072 = 0.00072479248046875` — exact in f32.
    let feathered = MatteSpec::window(WindowSpec::CENTRED.with_feather(10_000));
    let resolved = production_matte(&feathered).expect("an active matte");
    let corner = resolved.coverage(pixel_centre_uv(&frame, 0), aspect, raster[0]);
    assert_eq!(
        corner.to_bits(),
        (95.0_f32 / 131_072.0).to_bits(),
        "feather = 10000 at pixel (0, 0) must be exactly 95/131072, not {corner}"
    );
    assert_eq!(
        feathered
            .covered_pixels(&raster)
            .iter()
            .filter(|covered| **covered)
            .count(),
        CC5_RASTER_PIXELS,
        "with f = 1.0 the affected set is every pixel with D < 2, which is the whole raster"
    );
    //     At `D = 1` exactly the band is symmetric, so `w = 0.5`; at `D = 0`,
    //     `w = 1`. `D = 1` is not a pixel centre on this raster, so both are
    //     asserted on the resolved window at a synthetic uv.
    let wide_feather = feather_window(10_000);
    for (distance, expected) in [(0.0_f64, 1.0_f32), (1.0, 0.5)] {
        let weight = wide_feather.weight(feather_uv(distance), aspect);
        assert_eq!(
            weight.to_bits(),
            expected.to_bits(),
            "feather = 10000 at D = {distance} must be exactly {expected}, not {weight}"
        );
    }

    // (h) `hue_center = 35999`, the maximum: 359.99°. The §9.2.5 red anchor
    //     `e = (0.8, 0.2, 0.2)` has `M = r`, `C = 0.6`, `H = 0°`, so
    //     `dh = |0 - 359.99| = 359.99`, folded to `min(359.99, 0.01) = 0.01`.
    //     With `hue_width = 0` (its minimum): `0.01 > 0`, so `h = 0`.
    //     With `hue_width = 100` (1.00°): `0.01 ≤ 1`, so `h = 1`.
    //     This is the seam: a hue leg that could not wrap would report 359.99.
    const RED_ANCHOR: [f32; 3] = [0.8, 0.2, 0.2];
    // (i) `hue_softness = 18000`, the maximum: 180°. With `hue_center = 0`
    //     and `hue_width = 0`, `h = 1 - smoothstep(0, 180, dh)`. The anchor
    //     `e = (0.6, 0.8, 0.4)` has `M = g = 0.8`, `mn = 0.4`, `C = 0.4`, and
    //     `(b - r)/C = -0.5`, so `H = 60·(-0.5 + 2) = 90°` exactly. Then
    //     `t = 90/180 = 0.5`, `smoothstep = 0.25·(3 - 1) = 0.5`, and
    //     `h = 0.5` exactly.
    const GREEN_ANCHOR: [f32; 3] = [0.6, 0.8, 0.4];
    // (j) and (k), the saturation and luma band extremes.
    //     `e = (0.5, 0.5, 0.5)`: `C = 0`, so `S = 0`, and
    //     `Y = 0.5·(0.2126 + 0.7152 + 0.0722) = 0.5`.
    //     `e = (0.8, 0.0, 0.0)`: `mn = 0`, so `S = C/M = 1`.
    //     `e = (0.0, 0.0, 0.0)`: `M = 0`, so `S = 0` by the explicit zero
    //     rule, and `Y = 0`.
    const GREY_ANCHOR: [f32; 3] = [0.5, 0.5, 0.5];
    const PURE_RED_ANCHOR: [f32; 3] = [0.8, 0.0, 0.0];
    const BLACK_ANCHOR: [f32; 3] = [0.0, 0.0, 0.0];
    /// The hue leg switched off at its `18000` neutral, both bands wide open.
    const OPEN: QualifierSpec = QualifierSpec::NEUTRAL;
    let anchors: Vec<(&str, QualifierSpec, [f32; 3], f32)> = vec![
        // (h) the hue-centre maximum, both sides of the 0.01° seam.
        (
            "hue_center_max_width_min",
            QualifierSpec {
                hue_center: 35_999,
                hue_width: 0,
                ..OPEN
            },
            RED_ANCHOR,
            0.0,
        ),
        (
            "hue_center_max_width_one_degree",
            QualifierSpec {
                hue_center: 35_999,
                hue_width: 100,
                ..OPEN
            },
            RED_ANCHOR,
            1.0,
        ),
        // (i) the hue-softness maximum at a 90° separation.
        (
            "hue_softness_max_at_ninety_degrees",
            QualifierSpec {
                hue_center: 0,
                hue_width: 0,
                hue_softness: 18_000,
                ..OPEN
            },
            GREEN_ANCHOR,
            0.5,
        ),
        // (j) the saturation band at both extremes, hard and soft.
        //     `band(0, 0, 0, 0) = 1` — the low and high edges coincide at the
        //     minimum and the grey anchor sits exactly on them.
        (
            "saturation_band_pinned_at_min_selects_grey",
            QualifierSpec {
                sat_low: 0,
                sat_high: 0,
                ..OPEN
            },
            GREY_ANCHOR,
            1.0,
        ),
        //     …and rejects `S = 0.75`.
        (
            "saturation_band_pinned_at_min_rejects_colour",
            QualifierSpec {
                sat_low: 0,
                sat_high: 0,
                ..OPEN
            },
            RED_ANCHOR,
            0.0,
        ),
        //     `band(1, 1, 1, 0) = 1`: both edges at the maximum, and
        //     `e = (0.8, 0, 0)` has `S` exactly 1.
        (
            "saturation_band_pinned_at_max_selects_full_saturation",
            QualifierSpec {
                sat_low: 10_000,
                sat_high: 10_000,
                ..OPEN
            },
            PURE_RED_ANCHOR,
            1.0,
        ),
        //     Both edges at the maximum with softness at *its* maximum:
        //     `band(v, 1, 1, 1) = min(smoothstep(0, 1, v), 1 - smoothstep(1, 2, v))`.
        //     At `S = 0.75` the right factor is 1 and the left is
        //     `0.75²·(3 - 1.5) = 0.5625·1.5 = 0.84375` — exact in f32.
        (
            "saturation_softness_max_shoulder",
            QualifierSpec {
                sat_low: 10_000,
                sat_high: 10_000,
                sat_softness: 10_000,
                ..OPEN
            },
            RED_ANCHOR,
            0.843_75,
        ),
        //     Degenerate: `lo > hi` returns 0 with no clamp and no reorder.
        (
            "saturation_band_inverted_is_zero",
            QualifierSpec {
                sat_low: 10_000,
                sat_high: 0,
                ..OPEN
            },
            RED_ANCHOR,
            0.0,
        ),
        // (k) the luma band at both extremes.
        //     `band(0, 0, 0, 0) = 1` on the black anchor, `Y = 0`.
        (
            "luma_band_pinned_at_min_selects_black",
            QualifierSpec {
                luma_low: 0,
                luma_high: 0,
                ..OPEN
            },
            BLACK_ANCHOR,
            1.0,
        ),
        //     …and rejects `Y = 0.5`.
        (
            "luma_band_pinned_at_min_rejects_grey",
            QualifierSpec {
                luma_low: 0,
                luma_high: 0,
                ..OPEN
            },
            GREY_ANCHOR,
            0.0,
        ),
        //     Both edges at the maximum with softness at its maximum:
        //     `band(0.5, 1, 1, 1) = min(smoothstep(0, 1, 0.5), 1) = 0.25·2 = 0.5`.
        (
            "luma_softness_max_shoulder",
            QualifierSpec {
                luma_low: 10_000,
                luma_high: 10_000,
                luma_softness: 10_000,
                ..OPEN
            },
            GREY_ANCHOR,
            0.5,
        ),
        //     Degenerate: `lo > hi` returns 0.
        (
            "luma_band_inverted_is_zero",
            QualifierSpec {
                luma_low: 10_000,
                luma_high: 0,
                ..OPEN
            },
            GREY_ANCHOR,
            0.0,
        ),
    ];
    let mut recorded_anchors = Vec::new();
    for (label, qualifier, encoded, expected) in anchors {
        let spec = MatteSpec::qualifier(qualifier);
        let resolved = production_matte(&spec).expect("a qualifier-only matte is active");
        let linear = qualifier_input(encoded);
        // uv is irrelevant to a window-free matte; state that by probing two.
        let first = resolved.coverage([0.25, 0.25], aspect, linear);
        let second = resolved.coverage([0.75, 0.6], aspect, linear);
        assert_eq!(
            first.to_bits(),
            second.to_bits(),
            "{label}: a window-free matte must not depend on uv"
        );
        assert!(
            (f64::from(first) - f64::from(expected)).abs() <= f64::from(QUALIFIER_ANCHOR_TOLERANCE),
            "{label}: the production coverage is {first} and the hand derivation gives {expected}"
        );
        // The independent transcription must land on the same literal, so the
        // derivation is checked twice rather than trusted once.
        let transcribed = spec_qualifier_weight_f64(&qualifier, linear);
        assert!(
            (transcribed - f64::from(expected)).abs() <= f64::from(QUALIFIER_ANCHOR_TOLERANCE),
            "{label}: the §2.4 transcription gives {transcribed} and the hand derivation gives \
             {expected}"
        );
        recorded_anchors.push(json!({
            "case": label,
            "encoded": encoded,
            "expected": expected,
            "production": first,
            "transcription": transcribed,
        }));
    }
    // The two selector premises the (h)–(k) derivations rest on, asserted from
    // the transcription rather than assumed.
    let (red_saturation, _, red_hue) = spec_selectors_f64(qualifier_input(RED_ANCHOR));
    assert!((red_saturation - 0.75).abs() <= 1e-6, "{red_saturation}");
    assert!(
        red_hue.is_some_and(|hue| hue.abs() <= 1e-4),
        "the red anchor's hue is 0°, not {red_hue:?}"
    );
    let (_, grey_luma, grey_hue) = spec_selectors_f64(qualifier_input(GREY_ANCHOR));
    assert!((grey_luma - 0.5).abs() <= 1e-6, "{grey_luma}");
    assert_eq!(grey_hue, None, "an achromatic anchor has no hue");
    let (_, _, green_hue) = spec_selectors_f64(qualifier_input(GREEN_ANCHOR));
    assert!(
        green_hue.is_some_and(|hue| (hue - 90.0).abs() <= 1e-4),
        "the green anchor's hue is 90°, not {green_hue:?}"
    );

    // --- leg 3: the GPU agrees at an off-frame centre and at half_width = 1 -
    //
    // Both cases pair the extreme window with an ordinary one under a union,
    // so the expected raster is non-uniform and a degenerate window that
    // returned `NaN` — which `max` would propagate — could not pass.
    let gpu = fallback_gpu();
    let compositor = Compositor::new(gpu.context());
    let right_half = WindowSpec::CENTRED.with_centre(7_500, 5_000);
    let gpu_cases = [
        (
            // The off-frame window contributes exactly 0 at every pixel, so
            // `max` leaves the right-hand window alone: columns 32..=63 by
            // rows 9..=26, 32 × 18 = 576 pixels.
            "off_frame_centre_union",
            MatteSpec::window(off_frame_left)
                .with_windows(vec![off_frame_left, right_half], COMBINE_UNION),
            (32_u32, 63_u32, 9_u32, 26_u32),
        ),
        (
            // The 1-basis-point window contributes exactly 0, so `max` leaves
            // the centred window alone: columns 16..=47 by rows 9..=26.
            "hair_half_width_union",
            MatteSpec::window(hair_width)
                .with_windows(vec![hair_width, WindowSpec::CENTRED], COMBINE_UNION),
            (16, 47, 9, 26),
        ),
    ];
    let mut gpu_hashes = Vec::new();
    for (label, spec, expected_box) in gpu_cases {
        let covered = spec.covered_pixels(&raster);
        let count = covered.iter().filter(|covered| **covered).count();
        assert_eq!(
            count, CENTRED_WINDOW_PIXELS,
            "{label}: the §2.3 transcription covers {count} pixels, not 576"
        );
        assert_eq!(
            covered_bounding_box(&covered),
            Some(expected_box),
            "{label}: the covered bounding box is not the hand-derived rectangle"
        );
        let (left, right, top, bottom) = expected_box;
        for (index, is_covered) in covered.iter().enumerate() {
            let x = index as u32 % CC5_RASTER_WIDTH;
            let y = index as u32 / CC5_RASTER_WIDTH;
            assert_eq!(
                *is_covered,
                (left..=right).contains(&x) && (top..=bottom).contains(&y),
                "{label}: pixel ({x}, {y}) is not the hand-derived rectangle"
            );
        }
        let graded = gain_wheels(1, 1_500, Some(&spec));
        let coverage = gpu_coverage(
            &compositor,
            &frame,
            std::slice::from_ref(&graded),
            EffectId(1),
        );
        assert_coverage_matches(&coverage, &spec.coverage_values(&raster), true, label);
        for (index, code) in coverage.iter().enumerate() {
            assert_eq!(
                *code == u8::MAX,
                covered[index],
                "{label}: the GPU coverage byte at pixel {index} is {code}"
            );
        }
        gpu_hashes.push(json!({"case": label, "output_hash_sha256": output_hash(&coverage)}));
    }

    emit_cc5_evidence(
        "cc5_control_bounds",
        gpu.backend(),
        gpu.lane.id(),
        json!({
            "rule": "CC5 §9.0.2: every control at minimum, maximum, and a representative interior value has a numeric expected value",
            "controls_swept": MATTE_PARAMETER_COUNT,
            "probe_pixels": CONTROL_PROBE_PIXELS.map(|(x, y)| json!([x, y])),
            "swept_window": format!("{SWEPT_WINDOW:?}"),
            "swept_qualifier": format!("{SWEPT_QUALIFIER:?}"),
        }),
        (CC5_RASTER_WIDTH, CC5_RASTER_HEIGHT),
        json_hash(&json!(gpu_hashes)),
        json!({
            "swept_control_values": swept_values,
            "sweep_tolerance": CONTROL_SWEEP_TOLERANCE,
            "worst_sweep_divergence": worst_divergence,
            "worst_sweep_case": worst_label,
            "inactive_bounds": inactive_bounds,
            "qualifier_anchors": recorded_anchors,
            "gpu_coverage_rasters": gpu_hashes,
            "feather_max_corner_weight": 95.0_f64 / 131_072.0,
        }),
    );
}

/// CC5 §2.3's defensive clause: `hw <= 0` or `hh <= 0` makes that window's
/// `w = 0`. No error, no clamp.
///
/// The descriptor minimum for both half extents is `1`, and every keyframe
/// value is validated against the descriptor, so this state is **unreachable
/// through operations** — it exists for a hostile or future buffer. The params
/// are therefore constructed directly rather than driven through an `Effect`,
/// which would clamp them back into range before the window ever resolved.
#[test]
fn cc5_degenerate_window_half_extents_weigh_exactly_zero() {
    // Reachable-through-operations premise, stated so the direct construction
    // below is justified rather than convenient.
    let descriptor = effect_descriptor("color_wheels").expect("the descriptor exists");
    for suffix in ["half_width_basis_points", "half_height_basis_points"] {
        let name = format!("matte_window0_{suffix}");
        let parameter = descriptor
            .parameter(&name)
            .unwrap_or_else(|| panic!("color_wheels must carry {name}"));
        assert_eq!(parameter.min, 1, "{name} minimum");
    }
    let clamped = gain_wheels(
        1,
        1_500,
        Some(&MatteSpec::window(WindowSpec {
            hw: 0,
            hh: 0,
            ..WindowSpec::CENTRED
        })),
    );
    let params = MatteParams::from_effect(&clamped);
    let window = params.window(0).expect("window 0 exists");
    assert_eq!(
        (window.half_width_bp, window.half_height_bp),
        (1, 1),
        "the descriptor clamp is what makes a zero half extent unreachable"
    );

    let probes = [
        [0.5_f32, 0.5],
        [0.0, 0.0],
        [1.0, 1.0],
        [0.25, 0.75],
        [0.507_812_5, 0.486_111_1],
    ];
    let mut cases = 0_usize;
    for (hw, hh) in [
        (0_i64, 2_500_i64),
        (2_500, 0),
        (0, 0),
        (-1, 2_500),
        (2_500, -1),
        (-10_000, -10_000),
    ] {
        for shape in [SHAPE_RECT, SHAPE_ELLIPSE] {
            for feather in [0_i64, 2_500, 10_000] {
                for invert in [0_i64, 1] {
                    let window = MatteWindow::from_params(&kinewright_core::MatteWindowParams {
                        shape_token: shape,
                        center_x_bp: 5_000,
                        center_y_bp: 5_000,
                        half_width_bp: hw,
                        half_height_bp: hh,
                        rotation_cd: 0,
                        feather_bp: feather,
                        invert,
                    });
                    // The invert is a true complement, so a degenerate window
                    // is exactly 0 and its complement exactly 1 — no epsilon,
                    // no NaN, and no `-0.0`, which `to_bits` would catch.
                    let expected: f32 = if invert == 1 { 1.0 } else { 0.0 };
                    for uv in probes {
                        let weight = window.weight(uv, SPEC_RASTER_ASPECT_F64 as f32);
                        assert_eq!(
                            weight.to_bits(),
                            expected.to_bits(),
                            "a window with half extents ({hw}, {hh}), shape {shape}, feather \
                             {feather}, invert {invert} weighs {weight} at {uv:?}"
                        );
                    }
                    cases += 1;
                }
            }
        }
    }
    assert_eq!(cases, 6 * 2 * 3 * 2);

    // And a degenerate window cannot poison a combine: `max` with an exact 0
    // and `min` with an exact 1 are both the identity, which is only true
    // because the weights above are exact.
    let degenerate = MatteWindow::from_params(&kinewright_core::MatteWindowParams {
        half_width_bp: 0,
        ..kinewright_core::MatteWindowParams::NEUTRAL
    });
    let ordinary = MatteWindow::from_params(&kinewright_core::MatteWindowParams::NEUTRAL);
    let aspect = SPEC_RASTER_ASPECT_F64 as f32;
    for uv in probes {
        let a = degenerate.weight(uv, aspect);
        let b = ordinary.weight(uv, aspect);
        assert_eq!(a.max(b).to_bits(), b.to_bits(), "union at {uv:?}");
        assert_eq!(a.min(b).to_bits(), a.to_bits(), "intersection at {uv:?}");
    }
}

// ---------------------------------------------------------------------------
// §9.2.3: feather.
// ---------------------------------------------------------------------------

/// A window whose half-height is a dyadic `0.3125`, so a dyadic uv offset
/// produces an **exactly representable** distance `D = dy / 0.3125`.
///
/// The anchors are stated in `D`, so they can only be asserted bit-exactly if
/// `D` itself is exact; `0.2 / 0.25` would not be.
const FEATHER_WINDOW: WindowSpec = WindowSpec {
    shape: SHAPE_RECT,
    cx: 5_000,
    cy: 5_000,
    hw: 3_125,
    hh: 3_125,
    rotation: 0,
    feather: 0,
    invert: 0,
};

/// The uv that puts the §9.2.3 anchor `distance` on the window's `y` axis.
///
/// `u.x = 0.5` makes `n.x` exactly zero, so a rect's `max(|n.x|, |n.y|)` is
/// `|n.y|` and the anchor is the distance the contract names.
fn feather_uv(distance: f64) -> [f32; 2] {
    let offset = distance * 0.3125;
    let uv = [0.5_f32, (0.5 + offset) as f32];
    assert_eq!(
        f64::from(uv[1] - 0.5),
        offset,
        "the §9.2.3 anchor offset {offset} must be exactly representable"
    );
    uv
}

fn feather_window(feather: i64) -> MatteWindow {
    MatteWindow::from_params(
        MatteParams::from_effect(&gain_wheels(
            1,
            1_500,
            Some(&MatteSpec::window(FEATHER_WINDOW.with_feather(feather))),
        ))
        .window(0)
        .expect("window 0 resolves"),
    )
}

/// CC5 §9.2.3. The feather anchors, the complement symmetry, the exact
/// affected set `{D < 1 + f}`, and the mandatory `f == 0` hard branch.
#[test]
fn cc5_feather_anchors_and_symmetry_match_the_contract() {
    let aspect = CC5_RASTER_WIDTH as f32 / CC5_RASTER_HEIGHT as f32;

    // --- the non-dyadic feather = 4000 case -------------------------------
    let soft = feather_window(4_000);
    let w_08 = soft.weight(feather_uv(0.8), aspect);
    let w_10 = soft.weight(feather_uv(1.0), aspect);
    let w_12 = soft.weight(feather_uv(1.2), aspect);
    assert_eq!(
        w_08.to_bits(),
        0.843_75_f32.to_bits(),
        "w(D = 0.8) with f = 0.4 is exactly 0.84375"
    );
    assert_eq!(w_10.to_bits(), 0.5_f32.to_bits(), "w(D = 1) is exactly 0.5");
    let w_12_error = (w_12 - 0.156_25).abs();
    assert!(
        w_12_error <= FEATHER_NON_DYADIC_TOLERANCE,
        "w(D = 1.2) is {w_12}, {w_12_error} from 0.15625; f = 0.4 is not dyadic so 1 ± f each \
         round and this anchor lands one ULP off"
    );
    assert!(
        w_12_error > 0.0,
        "the f = 0.4 case is the non-dyadic one; if it became exact the dyadic control case below \
         would be measuring the same thing twice"
    );

    // --- the dyadic feather = 2500 control case ---------------------------
    let dyadic = feather_window(2_500);
    assert_eq!(
        dyadic.weight(feather_uv(0.875), aspect).to_bits(),
        0.843_75_f32.to_bits()
    );
    assert_eq!(
        dyadic.weight(feather_uv(1.0), aspect).to_bits(),
        0.5_f32.to_bits()
    );
    assert_eq!(
        dyadic.weight(feather_uv(1.125), aspect).to_bits(),
        0.156_25_f32.to_bits()
    );

    // --- the complement symmetry `w(1 − δ) + w(1 + δ) = 1` ----------------
    let mut symmetry = Vec::new();
    for (label, window) in [("feather_4000", &soft), ("feather_2500", &dyadic)] {
        for delta in [0.1_f64, 0.2, 0.4] {
            let low = window.weight(feather_uv(1.0 - delta), aspect);
            let high = window.weight(feather_uv(1.0 + delta), aspect);
            let sum = low + high;
            assert!(
                (sum - 1.0).abs() <= FEATHER_NON_DYADIC_TOLERANCE,
                "{label}: w(1 − {delta}) + w(1 + {delta}) = {sum}, which must be 1 so that invert \
                 is a true complement"
            );
            symmetry.push(json!({"window": label, "delta": delta, "sum": sum}));
        }
    }

    // --- the `f == 0` hard branch -----------------------------------------
    let hard = feather_window(0);
    assert_eq!(hard.weight(feather_uv(0.9375), aspect), 1.0);
    assert_eq!(hard.weight(feather_uv(1.0), aspect), 1.0);
    assert_eq!(hard.weight(feather_uv(1.0625), aspect), 0.0);

    // --- the affected set is exactly `{D < 1.4}` --------------------------
    let raster = cc5_field_raster();
    let frame = frame_of(&raster);
    // Quarter-frame extents put the whole `f = 0.4` band on the raster, so
    // the affected set really is bounded by `D < 1.4` rather than by the
    // raster edge.
    let feathered = WindowSpec::CENTRED.with_feather(4_000);
    let matte = MatteSpec::window(feathered);
    let covered = matte.covered_pixels(&raster);
    let mut margin = f64::INFINITY;
    for (index, covered) in covered.iter().enumerate() {
        let distance = spec_window_distance_f64(
            &feathered,
            spec_pixel_centre_uv_f64(index),
            SPEC_RASTER_ASPECT_F64,
        );
        assert_eq!(
            *covered,
            distance < 1.4,
            "the affected set must be exactly {{D < 1 + f}}; pixel {index} has D = {distance}"
        );
        margin = margin.min((distance - 1.4).abs());
    }
    assert!(
        margin > 1.0e-3,
        "no pixel centre may sit within f32 noise of D = 1.4; smallest margin is {margin}"
    );
    let gpu = fallback_gpu();
    let compositor = Compositor::new(gpu.context());
    let covered_count = covered.iter().filter(|covered| **covered).count();
    assert_matte_case(
        &compositor,
        &frame,
        &raster,
        &matte,
        covered_count,
        "feather_4000_affected_set",
    );

    emit_cc5_evidence(
        "cc5_feather",
        CPU_REFERENCE_BACKEND,
        CPU_REFERENCE_LANE,
        json!({
            "anchor_window": "rect, centre (5000, 5000), half extents 3125/3125",
            "feathers": [0, 2500, 4000],
            "affected_set_window": "rect 2500/2500, feather 4000",
        }),
        CC5_RESOLUTION,
        json_hash(&json!(symmetry)),
        json!({
            "w_0_8": w_08,
            "w_1_0": w_10,
            "w_1_2": w_12,
            "w_1_2_error": w_12_error,
            "symmetry": symmetry,
            "affected_pixels": covered_count,
            "smallest_distance_margin_to_1_4": margin,
        }),
    );
}

// ---------------------------------------------------------------------------
// §9.2.4: combine.
// ---------------------------------------------------------------------------

/// CC5 §9.2.4. Union and intersection are hand-derived by inclusion–exclusion
/// and asserted on the CPU reference and on the GPU, and a per-window invert
/// inside a union is asserted separately.
#[test]
fn cc5_window_combine_is_hand_derived_on_cpu_and_gpu() {
    let raster = cc5_field_raster();
    let frame = frame_of(&raster);
    let gpu = fallback_gpu();
    let compositor = Compositor::new(gpu.context());

    // Window A covers columns 16..=47; window B, centred at 7500, covers
    // columns 32..=63. Both cover rows 9..=26, so the overlap is columns
    // 32..=47 — 16 columns × 18 rows = 288 pixels.
    let a = WindowSpec::CENTRED;
    let b = WindowSpec::CENTRED.with_centre(7_500, 5_000);
    let union = MatteSpec::window(a).with_windows(vec![a, b], COMBINE_UNION);
    let intersection = MatteSpec::window(a).with_windows(vec![a, b], COMBINE_INTERSECTION);

    let union_covered = assert_matte_case(&compositor, &frame, &raster, &union, 864, "union");
    let intersection_covered = assert_matte_case(
        &compositor,
        &frame,
        &raster,
        &intersection,
        288,
        "intersection",
    );
    assert_eq!(covered_bounding_box(&union_covered), Some((16, 63, 9, 26)));
    assert_eq!(
        covered_bounding_box(&intersection_covered),
        Some((32, 47, 9, 26))
    );
    // 576 + 576 − 288 = 864, and the two sets are related by inclusion.
    assert_eq!(576 + 576 - 288, 864);
    for index in 0..CC5_RASTER_PIXELS {
        if intersection_covered[index] {
            assert!(
                union_covered[index],
                "the intersection must lie in the union"
            );
        }
    }

    // --- a per-window invert inside a union -------------------------------
    // `A ∪ ¬B` is everything except `B \ A`: 2304 − (576 − 288) = 2016.
    let inverted = MatteSpec::window(a).with_windows(vec![a, b.inverted()], COMBINE_UNION);
    let inverted_covered = assert_matte_case(
        &compositor,
        &frame,
        &raster,
        &inverted,
        2_016,
        "union_with_inverted_window",
    );
    assert_eq!(
        inverted_covered
            .iter()
            .zip(&union_covered)
            .filter(|(inverted, union)| !**inverted && **union)
            .count(),
        288,
        "the pixels the inverted window removes from the union are exactly B \\ A"
    );

    emit_cc5_evidence(
        "cc5_combine",
        gpu.backend(),
        gpu.lane.id(),
        json!({
            "window_a": "rect, centre (5000, 5000), half extents 2500/2500",
            "window_b": "rect, centre (7500, 5000), half extents 2500/2500",
            "combines": ["union", "intersection", "union with window B inverted"],
        }),
        CC5_RESOLUTION,
        json_hash(&json!({"union": 864, "intersection": 288, "inverted": 2016})),
        json!({
            "window_a_pixels": 576,
            "window_b_pixels": 576,
            "overlap_pixels": 288,
            "union_pixels": 864,
            "intersection_pixels": 288,
            "union_with_inverted_window_pixels": 2_016,
        }),
    );
}

// ---------------------------------------------------------------------------
// §9.2.5: the qualifier.
// ---------------------------------------------------------------------------

/// The linear triple whose `grade709` encoding is `e`, fed to the qualifier
/// exactly as CC5 §9.2.5 specifies.
fn qualifier_input(e: [f32; 3]) -> [f32; 3] {
    e.map(grade709_decode)
}

fn resolved_qualifier(spec: QualifierSpec) -> crate::color_pipeline::MatteQualifier {
    crate::color_pipeline::MatteQualifier::from_params(
        &MatteParams::from_effect(&gain_wheels(1, 1_500, Some(&MatteSpec::qualifier(spec))))
            .qualifier,
    )
}

/// CC5 §9.2.5. Every qualifier anchor — hue, wraparound, the achromatic rule
/// on both sides of the `18000` escape, saturation softness, and the
/// degenerate band — has a hand-derived numeric expected value.
#[test]
fn cc5_qualifier_anchors_match_the_hand_derived_values() {
    // --- the selectors of the contract's two chromatic anchors ------------
    const RED_ANCHOR: [f32; 3] = [0.8, 0.2, 0.2];
    const WRAP_ANCHOR: [f32; 3] = [0.8, 0.2, 0.35];
    const GREY_ANCHOR: [f32; 3] = [0.5, 0.5, 0.5];
    let red = qualifier_input(RED_ANCHOR);
    let wrap = qualifier_input(WRAP_ANCHOR);
    let grey = qualifier_input(GREY_ANCHOR);

    let (saturation, luma, hue) = spec_selectors_f64(red);
    assert!((saturation - 0.75).abs() < 1.0e-6, "S = {saturation}");
    assert!((luma - 0.327_56).abs() < 1.0e-6, "Y = {luma}");
    assert!(hue.expect("chromatic").abs() < 1.0e-4, "H = {hue:?}");
    let (wrap_saturation, _, wrap_hue) = spec_selectors_f64(wrap);
    assert!((wrap_saturation - 0.75).abs() < 1.0e-6);
    assert!(
        (wrap_hue.expect("chromatic") - 345.0).abs() < 1.0e-4,
        "the wraparound anchor's hue is 345°, not {wrap_hue:?}"
    );
    let (grey_saturation, grey_luma, grey_hue) = spec_selectors_f64(grey);
    assert_eq!(grey_saturation, 0.0);
    assert!((grey_luma - 0.5).abs() < 1.0e-6);
    assert_eq!(grey_hue, None, "C == 0 leaves the hue undefined");

    // --- the anchor table --------------------------------------------------
    struct Anchor {
        label: &'static str,
        input: [f32; 3],
        qualifier: QualifierSpec,
        expected: f32,
    }
    let anchors = [
        Anchor {
            label: "hue_band_hit",
            input: red,
            qualifier: QualifierSpec {
                hue_center: 0,
                hue_width: 3_000,
                ..QualifierSpec::NEUTRAL
            },
            expected: 1.0,
        },
        Anchor {
            label: "hue_wraparound_hit",
            input: wrap,
            qualifier: QualifierSpec {
                hue_center: 35_000,
                hue_width: 1_000,
                ..QualifierSpec::NEUTRAL
            },
            expected: 1.0,
        },
        Anchor {
            // dh = min(343, 17) = 17; t = (17 − 10)/10 = 0.7;
            // smoothstep = 0.49·1.6 = 0.784; h = 1 − 0.784 = 0.216.
            label: "hue_wraparound_shoulder",
            input: wrap,
            qualifier: QualifierSpec {
                hue_center: 200,
                hue_width: 1_000,
                hue_softness: 1_000,
                ..QualifierSpec::NEUTRAL
            },
            expected: 0.216,
        },
        Anchor {
            label: "achromatic_excluded_by_a_named_hue",
            input: grey,
            qualifier: QualifierSpec {
                hue_center: 0,
                hue_width: 3_000,
                ..QualifierSpec::NEUTRAL
            },
            expected: 0.0,
        },
        Anchor {
            label: "achromatic_included_when_the_hue_leg_is_disabled",
            input: grey,
            qualifier: QualifierSpec::NEUTRAL,
            expected: 1.0,
        },
        Anchor {
            // S = 0.75 against band 0.8..1.0 with softness 0.1:
            // min(smoothstep(0.7, 0.8, 0.75), 1) = min(0.5, 1) = 0.5.
            label: "saturation_shoulder",
            input: red,
            qualifier: QualifierSpec {
                sat_low: 8_000,
                sat_high: 10_000,
                sat_softness: 1_000,
                ..QualifierSpec::NEUTRAL
            },
            expected: 0.5,
        },
        Anchor {
            label: "degenerate_saturation_band",
            input: red,
            qualifier: QualifierSpec {
                sat_low: 9_000,
                sat_high: 1_000,
                ..QualifierSpec::NEUTRAL
            },
            expected: 0.0,
        },
    ];
    let mut measured = Vec::new();
    for anchor in &anchors {
        let actual = resolved_qualifier(anchor.qualifier).weight(anchor.input);
        assert!(
            (actual - anchor.expected).abs() <= QUALIFIER_ANCHOR_TOLERANCE,
            "qualifier anchor {}: expected {}, measured {actual}",
            anchor.label,
            anchor.expected
        );
        // And the independent f64 transcription reproduces the same anchor,
        // so the number above is the contract's rather than the code's.
        let spec = spec_qualifier_weight_f64(&anchor.qualifier, anchor.input);
        assert!(
            (spec - f64::from(anchor.expected)).abs() <= f64::from(QUALIFIER_ANCHOR_TOLERANCE),
            "the §2.4 transcription gives {spec} for anchor {}",
            anchor.label
        );
        measured.push(json!({
            "anchor": anchor.label,
            "expected": anchor.expected,
            "measured": actual,
            "transcription": spec,
        }));
    }

    // --- the degenerate band is reported, not clamped ---------------------
    let degenerate = gain_wheels(
        1,
        1_500,
        Some(&MatteSpec::qualifier(QualifierSpec {
            sat_low: 9_000,
            sat_high: 1_000,
            ..QualifierSpec::NEUTRAL
        })),
    );
    let mut document = cc5_document();
    document.tracks[0].clips[0].effects = vec![degenerate];
    let report = kinewright_core::qa_document(&document);
    let issue = report
        .issues
        .iter()
        .find(|issue| issue.code == "matte_band_inverted_by_automation")
        .expect("CC5 §2.6 reports a degenerate band rather than clamping it");
    assert_eq!(issue.severity, kinewright_core::QaSeverity::Warning);
    assert!(
        issue.message.contains("saturation"),
        "the report must name the band: {}",
        issue.message
    );

    emit_cc5_evidence(
        "cc5_qualifier",
        CPU_REFERENCE_BACKEND,
        CPU_REFERENCE_LANE,
        json!({
            "anchors": ["e = (0.8, 0.2, 0.2)", "e = (0.8, 0.2, 0.35)", "e = (0.5, 0.5, 0.5)"],
        }),
        CC5_RESOLUTION,
        json_hash(&json!(measured)),
        json!({
            "red_anchor": {"saturation": saturation, "luma": luma, "hue": hue},
            "wraparound_anchor": {"saturation": wrap_saturation, "hue": wrap_hue},
            "achromatic_anchor": {"saturation": grey_saturation, "luma": grey_luma},
            "weights": measured,
        }),
    );
}

// ---------------------------------------------------------------------------
// §9.2.6: mix and invert.
// ---------------------------------------------------------------------------

fn resolved_matte(matte: &MatteSpec) -> Option<Matte> {
    Matte::from_params(&MatteParams::from_effect(&gain_wheels(
        1,
        1_500,
        Some(matte),
    )))
}

/// CC5 §9.2.6. `matte_mix` scales the coverage, `matte_invert` complements it,
/// and `matte_mix = 0` makes the node inactive and losslessly identical to
/// removing it.
#[test]
fn cc5_mix_and_invert_scale_the_coverage_exactly() {
    let aspect = CC5_RASTER_WIDTH as f32 / CC5_RASTER_HEIGHT as f32;

    // --- m_raw = 0.5 at D = 1, scaled by mix = 6000 -----------------------
    let feathered = FEATHER_WINDOW.with_feather(2_500);
    let scaled = MatteSpec::window(feathered).with_mix(6_000);
    let matte = resolved_matte(&scaled).expect("an enabled, non-neutral matte resolves");
    let uv = feather_uv(1.0);
    let sample = [0.25_f32, 0.5, 0.75];
    let coverage = matte.coverage(uv, aspect, sample);
    assert!(
        (coverage - 0.3).abs() <= 1.0e-6,
        "m_raw = 0.5 at mix 6000 must resolve to m = 0.3, measured {coverage}"
    );

    // --- `out = x + (node(x) − x)·m` on three raster samples --------------
    // The node output is the independent f32 transcription of CC3 §2.2 with
    // `slope = 1.5`, `offset = 0`, `power = 1`, not a call into the pipeline.
    let graded = gain_wheels(1, 1_500, Some(&scaled));
    let nodes = cpu_nodes(std::slice::from_ref(&graded));
    let mut blended = Vec::new();
    for sample in [[0.25_f32, 0.5, 0.75], [0.05, 0.1, 0.9], [0.6, 0.35, 0.2]] {
        let actual = apply_color_nodes_at(&nodes, sample, uv, aspect);
        for channel in 0..3 {
            let node = spec_wheels_apply_f32(1.5, 0.0, 1.0, sample[channel]);
            let expected = sample[channel] + (node - sample[channel]) * 0.3;
            assert!(
                (actual[channel] - expected).abs() <= 2.0e-6 * expected.abs().max(1.0),
                "channel {channel} of {sample:?}: expected {expected}, measured {}",
                actual[channel]
            );
        }
        blended.push(json!({"input": sample, "output": actual}));
    }

    // --- invert on m_raw = 0.15625 ----------------------------------------
    let inverted = MatteSpec::window(feathered).inverted();
    let inverted_matte = resolved_matte(&inverted).expect("an inverted matte resolves");
    let inverted_coverage = inverted_matte.coverage(feather_uv(1.125), aspect, sample);
    assert_eq!(
        inverted_coverage.to_bits(),
        0.843_75_f32.to_bits(),
        "matte_invert on m_raw = 0.15625 must be exactly 0.84375"
    );

    // --- mix = 0 makes the node inactive ----------------------------------
    let excluded = gain_wheels(
        1,
        1_500,
        Some(&MatteSpec::window(WindowSpec::CENTRED).with_mix(0)),
    );
    assert_eq!(
        color_node_inactive_reason(&excluded),
        Some(ColorNodeInactiveReason::MatteExcluded),
        "CC5 §2.6: matte_mix = 0 with the matte enabled excludes the whole node"
    );
    assert!(
        cpu_nodes(std::slice::from_ref(&excluded)).is_empty(),
        "an inactive node must not reach the CPU reference at all"
    );
    let raster = cc5_field_raster();
    let frame = frame_of(&raster);
    let gpu = fallback_gpu();
    let compositor = Compositor::new(gpu.context());
    let with_excluded = gpu_linear(&compositor, &frame, std::slice::from_ref(&excluded), None);
    let without = gpu_linear(&compositor, &frame, &[], None);
    assert_eq!(
        with_excluded
            .iter()
            .map(|value| value.to_bits())
            .collect::<Vec<_>>(),
        without
            .iter()
            .map(|value| value.to_bits())
            .collect::<Vec<_>>(),
        "a zero-mix matte must be losslessly identical to removing the node, bit for bit"
    );

    emit_cc5_evidence(
        "cc5_mix_and_invert",
        gpu.backend(),
        gpu.lane.id(),
        json!({
            "window": "rect, centre (5000, 5000), half extents 3125/3125, feather 2500",
            "mixes": [0, 6_000, 10_000],
            "invert": [0, 1],
        }),
        CC5_RESOLUTION,
        json_hash(&json!(blended)),
        json!({
            "coverage_at_mix_6000": coverage,
            "inverted_coverage": inverted_coverage,
            "blended_samples": blended,
        }),
    );
}

// ---------------------------------------------------------------------------
// §9.2.7: keyframed window motion.
// ---------------------------------------------------------------------------

/// The clip-local frame the §9.2.7 motion ends on.
const KEYFRAME_LAST_FRAME: i64 = 99;

/// The integer-linear keyframe value at `frame`, transcribed from the
/// automation curve's own arithmetic rather than read back from it.
///
/// `linear = offset·10⁶ / span` truncates toward zero and the delta is applied
/// with a round-half-away-from-zero division, exactly as CC5's keyframes are
/// evaluated for every other parameter.
fn spec_linear_keyframe(start: i64, end: i64, frame: i64, span: i64) -> i64 {
    if frame <= 0 {
        return start;
    }
    if frame >= span {
        return end;
    }
    let linear = i128::from(frame) * 1_000_000 / i128::from(span);
    let delta = i128::from(end - start) * linear;
    let rounded = if delta >= 0 {
        (delta + 500_000) / 1_000_000
    } else {
        (delta - 500_000) / 1_000_000
    };
    start + rounded as i64
}

/// CC5 §9.2.7. A `Linear` keyframe pair moves the covered set from one half of
/// the frame to the other, containment holds at every frame independently, the
/// two extreme sets are disjoint, and a `Linear` keyframe on a token is
/// rejected.
#[test]
fn cc5_keyframed_window_motion_moves_the_covered_set() {
    let mut document = cc5_document();
    let matte = MatteSpec::window(WindowSpec::CENTRED);
    document.tracks[0].clips[0].effects = vec![gain_wheels(1, 1_500, Some(&matte))];
    let before = document.clone();

    kinewright_core::Operation::SetEffectKeyframes {
        clip: ClipId(1),
        effect: EffectId(1),
        name: "matte_window0_center_x_basis_points".to_owned(),
        curve: AutomationCurve {
            keyframes: vec![
                Keyframe {
                    at: TimeCode(0),
                    value: 2_500,
                    interpolation: KeyframeInterpolation::Linear,
                },
                Keyframe {
                    at: TimeCode(KEYFRAME_LAST_FRAME),
                    value: 7_500,
                    interpolation: KeyframeInterpolation::Linear,
                },
            ],
        },
    }
    .apply(&mut document)
    .expect("the window centre is fully keyframable with any interpolation");

    let raster = cc5_field_raster();
    let frame = frame_of(&raster);
    let gpu = fallback_gpu();
    let compositor = Compositor::new(gpu.context());
    let baseline = cpu_reference_linear(&frame, &[]);
    let mut first = Vec::new();
    let mut last = Vec::new();
    for local in 0..=KEYFRAME_LAST_FRAME {
        let evaluated = document.tracks[0].clips[0].effects[0].evaluated_at(TimeCode(local));
        let centre = spec_linear_keyframe(2_500, 7_500, local, KEYFRAME_LAST_FRAME);
        assert_eq!(
            evaluated.integer_parameter_at("matte_window0_center_x_basis_points", TimeCode(local)),
            Some(centre),
            "the evaluated centre at frame {local} must be the hand-derived {centre}"
        );
        let expected = MatteSpec::window(WindowSpec::CENTRED.with_centre(centre, 5_000));
        let covered = expected.covered_pixels(&raster);
        let cpu = cpu_reference_linear(&frame, &cpu_nodes(std::slice::from_ref(&evaluated)));
        assert_matte_containment(&cpu, &baseline, &covered, &format!("frame_{local}"));
        if local == 0 {
            first = covered;
        } else if local == KEYFRAME_LAST_FRAME {
            last = covered;
        }
    }

    // At frame 0 the centre is 2500, so `u.x ∈ [0, 0.5]` selects columns
    // 0..=31; at the last frame the centre is 7500 and it selects 32..=63.
    assert_eq!(covered_bounding_box(&first), Some((0, 31, 9, 26)));
    assert_eq!(covered_bounding_box(&last), Some((32, 63, 9, 26)));
    assert_eq!(first.iter().filter(|covered| **covered).count(), 576);
    assert_eq!(last.iter().filter(|covered| **covered).count(), 576);
    assert!(
        first
            .iter()
            .zip(&last)
            .all(|(first, last)| !(*first && *last)),
        "the frame 0 and last-frame covered sets must be disjoint"
    );

    // The GPU agrees at both ends, evaluated through the same keyframes.
    for (local, covered) in [(0_i64, &first), (KEYFRAME_LAST_FRAME, &last)] {
        let evaluated = document.tracks[0].clips[0].effects[0].evaluated_at(TimeCode(local));
        let coverage = gpu_coverage(
            &compositor,
            &frame,
            std::slice::from_ref(&evaluated),
            EffectId(1),
        );
        let expected = covered
            .iter()
            .map(|covered| f64::from(u8::from(*covered)))
            .collect::<Vec<_>>();
        assert_coverage_matches(&coverage, &expected, true, &format!("gpu_frame_{local}"));
    }

    // --- a token accepts `Hold` keyframes only ----------------------------
    let mut rejected = document.clone();
    let error = kinewright_core::Operation::SetEffectKeyframes {
        clip: ClipId(1),
        effect: EffectId(1),
        name: "matte_window0_shape_token".to_owned(),
        curve: AutomationCurve {
            keyframes: vec![
                Keyframe {
                    at: TimeCode(0),
                    value: 1,
                    interpolation: KeyframeInterpolation::Linear,
                },
                Keyframe {
                    at: TimeCode(10),
                    value: 2,
                    interpolation: KeyframeInterpolation::Hold,
                },
            ],
        },
    }
    .apply(&mut rejected)
    .expect_err("CC5 §5.1: a shape token accepts Hold keyframes only");
    assert_eq!(
        error,
        kinewright_core::OpError::NonHoldKeyframeParameter {
            effect: "color_wheels".to_owned(),
            name: "matte_window0_shape_token".to_owned(),
        }
    );
    assert_eq!(rejected, document, "a rejection is atomic");
    assert_ne!(
        document, before,
        "the accepted keyframes really were stored"
    );

    emit_cc5_evidence(
        "cc5_keyframed_window",
        gpu.backend(),
        gpu.lane.id(),
        json!({
            "parameter": "matte_window0_center_x_basis_points",
            "keyframes": [[0, 2_500], [KEYFRAME_LAST_FRAME, 7_500]],
            "interpolation": "linear",
        }),
        CC5_RESOLUTION,
        json_hash(&json!({
            "first": first.iter().filter(|covered| **covered).count(),
            "last": last.iter().filter(|covered| **covered).count(),
        })),
        json!({
            "frames_asserted": KEYFRAME_LAST_FRAME + 1,
            "first_frame_bounding_box": [0, 31, 9, 26],
            "last_frame_bounding_box": [32, 63, 9, 26],
            "disjoint": true,
            "hold_only_rejection": "NonHoldKeyframeParameter",
        }),
    );
}

// ---------------------------------------------------------------------------
// §9.2.13: buffer layout, limits, and the ABI.
// ---------------------------------------------------------------------------

const GRADE_HEADER_BYTES: usize = 16;
const GRADE_NODE_WORDS: usize = 16;
/// CC5 §3.1: `v11` is the matte payload word offset, record word 15.
const GRADE_NODE_MATTE_OFFSET_WORD: usize = 15;
/// CC5 §3.1: the matte block is 64 words, 256 bytes.
const MATTE_BLOCK_WORDS: usize = 64;
/// CC3 §3.2: four curves × 49 words.
const GRADE_CURVE_PAYLOAD_WORDS: usize = 4 * 49;
/// The CC5 §3.1 worst case, written out by hand.
const GRADE_BUFFER_WORST_CASE_BYTES: usize = 17_680;

fn grade_header_word(bytes: &[u8], index: usize) -> u32 {
    let offset = index * 4;
    u32::from_le_bytes(bytes[offset..offset + 4].try_into().expect("header word"))
}

fn grade_word(bytes: &[u8], word: usize) -> f32 {
    let offset = GRADE_HEADER_BYTES + word * 4;
    f32::from_le_bytes(
        bytes[offset..offset + 4]
            .try_into()
            .expect("f32-aligned node word"),
    )
}

/// A `color_curves` node whose master curve carries `points`.
fn curves_effect(id: u64, points: &[(i64, i64)], matte: Option<&MatteSpec>) -> Effect {
    let curve = kinewright_core::ColorCurveChannel::Master;
    let mut parameters = vec![(
        curve.point_count_parameter().to_owned(),
        points.len() as i64,
    )];
    for (index, (x, y)) in points.iter().enumerate() {
        parameters.push((
            curve.x_parameter(index).expect("point index").to_owned(),
            *x,
        ));
        parameters.push((
            curve.y_parameter(index).expect("point index").to_owned(),
            *y,
        ));
    }
    if let Some(matte) = matte {
        parameters.extend(matte.parameters());
    }
    color_node_effect(id, "color_curves", parameters)
}

/// CC5 §9.2.13. The worst-case buffer is exactly 17 680 bytes with
/// non-overlapping payload and matte regions, the negotiated binding holds it,
/// the binding count is still one, the ABI is 3, `technical_lut` never carries
/// a matte offset, and the layer quad's pixel aspect is the output raster
/// aspect at every scale.
#[test]
fn cc5_buffer_layout_limits_and_abi_constants_hold() {
    // --- the constants ----------------------------------------------------
    assert_eq!(COMPOSITOR_REQUIRED_STORAGE_BUFFER_BINDING_SIZE, 32_768);
    assert_eq!(COMPOSITOR_REQUIRED_STORAGE_BUFFERS_PER_SHADER_STAGE, 1);
    assert_eq!(
        16 + 16 * 64 + 16 * (4 * 49 * 4) + 16 * (64 * 4),
        GRADE_BUFFER_WORST_CASE_BYTES,
        "the CC5 §3.1 arithmetic, written out"
    );
    assert!(
        GRADE_BUFFER_WORST_CASE_BYTES as u64 <= COMPOSITOR_REQUIRED_STORAGE_BUFFER_BINDING_SIZE,
        "the negotiated binding must hold the worst case"
    );

    // --- sixteen curve-plus-matte nodes -----------------------------------
    let matte = MatteSpec::window(WindowSpec::CENTRED);
    let stack = (0..kinewright_core::COLOR_NODE_LIMIT_PER_LAYER)
        .map(|index| {
            curves_effect(
                index as u64 + 1,
                &[(0, 0), (5_000, 6_000), (10_000, 10_000)],
                Some(&matte),
            )
        })
        .collect::<Vec<_>>();
    let bytes = crate::compositor::grade_buffer_bytes_for(&stack, None, CC5_RESOLUTION, None)
        .expect("sixteen curve-plus-matte nodes fit the buffer");
    assert_eq!(bytes.len(), GRADE_BUFFER_WORST_CASE_BYTES);
    assert_eq!(grade_header_word(&bytes, 0), 16, "sixteen active nodes");
    assert_eq!(
        grade_header_word(&bytes, 2),
        3,
        "CC5 §3.1 takes GRADE_ABI_VERSION to 3"
    );
    assert_eq!(
        grade_header_word(&bytes, 3),
        0,
        "header.w is the matte-debug selector and is 0 for a normal render"
    );

    let mut regions: Vec<(usize, usize)> = Vec::new();
    for index in 0..kinewright_core::COLOR_NODE_LIMIT_PER_LAYER {
        let base = index * GRADE_NODE_WORDS;
        let payload = grade_word(&bytes, base + 1) as usize;
        let matte_offset = grade_word(&bytes, base + GRADE_NODE_MATTE_OFFSET_WORD) as usize;
        let expected_payload = 256 + index * (GRADE_CURVE_PAYLOAD_WORDS + MATTE_BLOCK_WORDS);
        assert_eq!(payload, expected_payload, "node {index} payload offset");
        assert_eq!(
            matte_offset,
            expected_payload + GRADE_CURVE_PAYLOAD_WORDS,
            "node {index} matte offset"
        );
        assert_ne!(matte_offset, 0, "a matte-carrying node must set v11");
        regions.push((payload, payload + GRADE_CURVE_PAYLOAD_WORDS));
        regions.push((matte_offset, matte_offset + MATTE_BLOCK_WORDS));
    }
    regions.sort_unstable();
    for pair in regions.windows(2) {
        assert!(pair[0].1 <= pair[1].0, "buffer regions overlap: {pair:?}");
    }
    assert_eq!(
        GRADE_HEADER_BYTES + regions.last().expect("regions").1 * 4,
        GRADE_BUFFER_WORST_CASE_BYTES
    );
    // The matte block itself carries the host-supplied raster aspect.
    let aspect_word = grade_word(&bytes, 256 + GRADE_CURVE_PAYLOAD_WORDS + 5);
    assert_eq!(
        aspect_word,
        CC5_RASTER_WIDTH as f32 / CC5_RASTER_HEIGHT as f32,
        "matte block word 5 is the host-supplied raster aspect a = W/H"
    );

    // --- `technical_lut` never carries a matte ----------------------------
    let store = TempDirectory::new("cc5-limits");
    let luts = fixture_luts(&store);
    let lut_stack = vec![
        color_node_effect(
            1,
            "technical_lut",
            vec![
                ("lut_asset_id".to_owned(), 1),
                ("input_encoding_token".to_owned(), 1),
            ],
        ),
        gain_wheels(2, 1_500, Some(&matte)),
    ];
    let lut_bytes =
        crate::compositor::grade_buffer_bytes_for(&lut_stack, Some(&luts), CC5_RESOLUTION, None)
            .expect("a technical LUT beside a matte-carrying node serializes");
    assert_eq!(
        grade_word(&lut_bytes, GRADE_NODE_MATTE_OFFSET_WORD),
        0.0,
        "CC5 §2.1: a technical_lut record's v11 is always 0"
    );
    assert_ne!(
        grade_word(&lut_bytes, GRADE_NODE_WORDS + GRADE_NODE_MATTE_OFFSET_WORD),
        0.0,
        "the matte-carrying node beside it still points at its block"
    );
    // A matte parameter is not even in the technical_lut descriptor, so
    // naming one there is the ordinary unknown-parameter rejection.
    let descriptor = effect_descriptor("technical_lut").expect("the descriptor exists");
    assert!(
        !descriptor
            .parameters
            .iter()
            .any(|parameter| is_matte_parameter(parameter.name)),
        "technical_lut must carry no matte parameter"
    );

    // --- the layer quad's pixel aspect is the output raster aspect --------
    // The quad is scaled uniformly in NDC, so a window that is circular in
    // pixels stays circular at every scale and its covered set simply scales.
    let raster = cc5_field_raster();
    let frame = frame_of(&raster);
    let gpu = fallback_gpu();
    let compositor = Compositor::new(gpu.context());
    let circular = MatteSpec::window(WindowSpec::PIXEL_SQUARE.with_shape(SHAPE_ELLIPSE));
    let mut scale_evidence = Vec::new();
    for scale_percent in [50_i64, 100, 200] {
        let effects = vec![
            color_node_effect(
                1,
                "transform",
                vec![("scale_percent".to_owned(), scale_percent)],
            ),
            gain_wheels(2, 1_500, Some(&circular)),
        ];
        let coverage = gpu_coverage(&compositor, &frame, &effects, EffectId(2));
        let covered = coverage.iter().map(|code| *code > 0).collect::<Vec<_>>();
        let box_ = covered_bounding_box(&covered).expect("the window is on screen at every scale");
        let width = box_.1 - box_.0 + 1;
        let height = box_.3 - box_.2 + 1;
        assert_eq!(
            width, height,
            "at scale {scale_percent}% the pixel-circular window measured {width} × {height}; the \
             quad's pixel aspect is not the output raster aspect"
        );
        scale_evidence.push(json!({
            "scale_percent": scale_percent,
            "coverage_bounding_box": [box_.0, box_.1, box_.2, box_.3],
            "width": width,
            "height": height,
        }));
    }

    emit_cc5_evidence(
        "cc5_buffer_and_limits",
        gpu.backend(),
        gpu.lane.id(),
        json!({
            "stack": "sixteen color_curves nodes, each carrying a one-window matte",
            "scales": [50, 100, 200],
        }),
        CC5_RESOLUTION,
        output_hash(&bytes),
        json!({
            "grade_buffer_worst_case_bytes": bytes.len(),
            "compositor_required_storage_buffer_binding_size":
                COMPOSITOR_REQUIRED_STORAGE_BUFFER_BINDING_SIZE,
            "compositor_required_storage_buffers_per_shader_stage":
                COMPOSITOR_REQUIRED_STORAGE_BUFFERS_PER_SHADER_STAGE,
            "grade_abi_version": grade_header_word(&bytes, 2),
            "raster_aspect_word": aspect_word,
            "layer_scale_cases": scale_evidence,
        }),
    );
}

// ---------------------------------------------------------------------------
// §9.2.8: CPU/GPU parity.
// ---------------------------------------------------------------------------

/// The §9.2.8 stack of a parity case, plus the baseline the containment gate
/// measures against.
struct ParityCase {
    label: &'static str,
    effects: Vec<Effect>,
    /// The nodes that stay active outside the matte, so "outside is
    /// unchanged" is measured against what the stack really does there.
    baseline: Vec<Effect>,
    matte: MatteSpec,
    library: bool,
}

/// The four §9.2.8 cases: windowed, qualifier, windowed + qualifier + feather,
/// and the five-kind full stack.
fn parity_cases() -> Vec<ParityCase> {
    let window = MatteSpec::window(WindowSpec::CENTRED);
    let qualifier = MatteSpec::qualifier(QualifierSpec {
        // A wide luma band with soft shoulders, so the parity raster's 24
        // levels straddle both shoulders rather than sitting in the interior.
        luma_low: 2_000,
        luma_high: 7_000,
        luma_softness: 1_500,
        ..QualifierSpec::NEUTRAL
    });
    let combined = MatteSpec::window(WindowSpec::CENTRED.with_feather(2_500))
        .with_qualifier(QualifierSpec {
            sat_low: 1_000,
            sat_high: 9_000,
            sat_softness: 2_000,
            ..QualifierSpec::NEUTRAL
        })
        .with_mix(6_000);
    let full_window = MatteSpec::window(WindowSpec::CENTRED);
    vec![
        ParityCase {
            label: "windowed",
            effects: vec![gain_wheels(1, 1_500, Some(&window))],
            baseline: Vec::new(),
            matte: window.clone(),
            library: false,
        },
        ParityCase {
            label: "qualifier",
            effects: vec![gain_wheels(1, 1_500, Some(&qualifier))],
            baseline: Vec::new(),
            matte: qualifier,
            library: false,
        },
        ParityCase {
            label: "windowed_qualifier_feathered",
            effects: vec![gain_wheels(1, 1_500, Some(&combined))],
            baseline: Vec::new(),
            matte: combined,
            library: false,
        },
        ParityCase {
            label: "full_stack",
            effects: vec![
                color_node_effect(
                    1,
                    "technical_lut",
                    vec![
                        ("lut_asset_id".to_owned(), 1),
                        ("input_encoding_token".to_owned(), 1),
                    ],
                ),
                color_node_effect(2, "primary_correction", {
                    let mut parameters = vec![
                        ("exposure_milli_stops".to_owned(), 750),
                        ("contrast_percent".to_owned(), 20),
                        ("saturation_percent".to_owned(), 15),
                    ];
                    parameters.extend(full_window.parameters());
                    parameters
                }),
                gain_wheels(3, 1_200, Some(&full_window)),
                curves_effect(
                    4,
                    &[(0, 0), (2_500, 1_800), (7_500, 8_200), (10_000, 10_000)],
                    Some(&full_window),
                ),
                color_node_effect(5, "creative_look", {
                    let mut parameters = vec![
                        ("lut_asset_id".to_owned(), 2),
                        ("mix_basis_points".to_owned(), 8_000),
                        ("input_encoding_token".to_owned(), 1),
                    ];
                    parameters.extend(full_window.parameters());
                    parameters
                }),
            ],
            // Outside the matte only the technical input transform runs, so
            // that is the baseline the containment gate must compare against.
            baseline: vec![color_node_effect(
                1,
                "technical_lut",
                vec![
                    ("lut_asset_id".to_owned(), 1),
                    ("input_encoding_token".to_owned(), 1),
                ],
            )],
            matte: full_window,
            library: true,
        },
    ]
}

/// Run the §9.2.8 parity cases on one lane and record the evidence.
fn assert_cc5_gpu_parity(gpu: &FixtureGpu) {
    let compositor = Compositor::new(gpu.context());
    let raster = cc5_parity_raster();
    let frame = frame_of(&raster);
    let directory = TempDirectory::new("cc5-parity");
    let library = fixture_luts(&directory);
    let mut recorded = Vec::new();
    let mut hashes = Vec::new();
    for case in parity_cases() {
        let library = case.library.then_some(&library);
        let nodes = library.map_or_else(
            || cpu_nodes(&case.effects),
            |library| cpu_nodes_with(&case.effects, library),
        );
        let baseline_nodes = library.map_or_else(
            || cpu_nodes(&case.baseline),
            |library| cpu_nodes_with(&case.baseline, library),
        );
        let expected_linear = cpu_reference_linear(&frame, &nodes);
        let expected_monitor = cpu_reference_monitor(&frame, &nodes);
        let baseline_linear = cpu_reference_linear(&frame, &baseline_nodes);
        let baseline_monitor = cpu_reference_monitor(&frame, &baseline_nodes);
        let covered = case.matte.covered_pixels(&raster);
        // CC5 §9.0.7's two-sided gate, on the CPU reference itself.
        let counts =
            assert_matte_containment(&expected_linear, &baseline_linear, &covered, case.label);
        assert_monitor_containment(&expected_monitor, &baseline_monitor, &covered, case.label);

        let actual_linear = gpu_linear(&compositor, &frame, &case.effects, library);
        let actual_monitor = gpu_monitor(&compositor, &frame, &case.effects, library);
        let linear = linear_parity_metrics(&actual_linear, &expected_linear);
        let monitor = abs_code_diff_rgb(&actual_monitor, &expected_monitor);
        assert!(
            linear.in_gamut_samples > 0,
            "case {} left the in-gamut §6.2 band empty: {linear:?}",
            case.label
        );
        assert!(
            monitor.max <= MONITOR_CPU_GPU_MAX,
            "GPU/CPU monitor max for {}: {monitor:?}",
            case.label
        );
        assert!(
            monitor.p99 <= MONITOR_CPU_GPU_P99,
            "GPU/CPU monitor P99 for {}: {monitor:?}",
            case.label
        );
        assert!(
            monitor.mean <= MONITOR_CPU_GPU_MEAN,
            "GPU/CPU monitor mean for {}: {monitor:?}",
            case.label
        );
        assert_linear_parity(&linear, case.label);
        // And the GPU obeys containment against its own baseline, so a
        // shader-side leak cannot hide behind the CPU/GPU tolerance.
        let gpu_baseline = gpu_linear(&compositor, &frame, &case.baseline, library);
        assert_matte_containment(
            &actual_linear,
            &gpu_baseline,
            &covered,
            &format!("{}_gpu", case.label),
        );

        // CC5 §12: the hue-sector-boundary divergence is recorded, not
        // assumed. Patterns 4, 5, and 6 (cyan, magenta, yellow) attain their
        // maximum in two channels at once, which is exactly the tie the
        // written branch order resolves.
        let mut sector_divergence = 0.0_f32;
        for index in 0..CC5_RASTER_PIXELS {
            let block_x = (index as u32 % CC5_RASTER_WIDTH) / CC5_PARITY_BLOCK_WIDTH;
            if !(4..=6).contains(&(block_x % 8)) {
                continue;
            }
            for channel in 0..3 {
                let difference = (actual_linear[index * 4 + channel]
                    - expected_linear[index * 4 + channel])
                    .abs();
                if difference.is_finite() {
                    sector_divergence = sector_divergence.max(difference);
                }
            }
        }

        hashes.push(output_hash(&actual_monitor));
        recorded.push(json!({
            "case": case.label,
            "containment": counts.as_json(),
            "monitor": {"max": monitor.max, "p99": monitor.p99, "mean": monitor.mean},
            "linear": linear.as_json(),
            "hue_sector_boundary_divergence": sector_divergence,
        }));
    }

    emit_cc5_evidence(
        "cc5_gpu_cpu_parity",
        gpu.backend(),
        gpu.lane.id(),
        json!({
            "raster": "cc5_parity_raster",
            "cases": ["windowed", "qualifier", "windowed_qualifier_feathered", "full_stack"],
            "tolerances": "CC1 §6.2, reused verbatim",
        }),
        CC5_RESOLUTION,
        json_hash(&json!(hashes)),
        json!({"cases": recorded}),
    );
}

/// CC5 §9.2.8 on the default software lane.
#[test]
fn cc5_gpu_compositor_matches_the_cpu_reference_on_software_fallback() {
    assert_cc5_gpu_parity(&fallback_gpu());
}

/// CC5 §9.2.8 on a physical adapter.
#[test]
#[ignore = "requires a physical supported GPU; run explicitly with --ignored --nocapture"]
fn cc5_gpu_compositor_matches_the_cpu_reference_on_hardware() {
    assert_cc5_gpu_parity(&hardware_gpu());
}

// ---------------------------------------------------------------------------
// §9.2.12: migration and the `mask` regression.
// ---------------------------------------------------------------------------

/// Every one of the 47 `matte_*` parameters at its descriptor neutral.
///
/// This is what a CC5 inspector writes when it resets a matte, and what a
/// migrated CC4 project acquires the first time anyone touches the section.
fn neutral_matte_parameters(effect_name: &str) -> Vec<(String, i64)> {
    let descriptor = effect_descriptor(effect_name).expect("a matte-capable descriptor");
    matte_parameter_names()
        .iter()
        .map(|name| {
            let parameter = descriptor
                .parameter(name)
                .unwrap_or_else(|| panic!("{effect_name} must carry {name}"));
            ((*name).to_owned(), parameter.neutral)
        })
        .collect()
}

fn to_bits(values: &[f32]) -> Vec<u32> {
    values.iter().map(|value| value.to_bits()).collect()
}

/// CC5 §9.2.12. A CC4 project renders bit-identically after CC5, and a clip
/// carrying **both** a `mask` and a matte-carrying node proves the two never
/// interact.
#[test]
fn cc5_migration_is_bit_identical_and_the_mask_never_interacts() {
    let raster = cc5_field_raster();
    let frame = frame_of(&raster);
    let gpu = fallback_gpu();
    let compositor = Compositor::new(gpu.context());

    // --- migration --------------------------------------------------------
    let cc4_era = gain_wheels(1, 1_500, None);
    assert_eq!(
        MATTE_PARAMETER_COUNT, 47,
        "CC5 §2.2 defines 47 matte parameters per matte-capable node"
    );
    let mut stored_neutral = vec![("gain_master_thousandths".to_owned(), 1_500_i64)];
    stored_neutral.extend(neutral_matte_parameters("color_wheels"));
    let migrated = color_node_effect(1, "color_wheels", stored_neutral);
    // The neutral matte has the master switch off, so it is inactive; so is an
    // *enabled* matte that selects everything at full strength (CC5 §2.6).
    let mut enabled_but_neutral = neutral_matte_parameters("color_wheels");
    for entry in &mut enabled_but_neutral {
        if entry.0 == "matte_enabled" {
            entry.1 = 1;
        }
    }
    enabled_but_neutral.push(("gain_master_thousandths".to_owned(), 1_500));
    let enabled_neutral = color_node_effect(1, "color_wheels", enabled_but_neutral);

    for (label, candidate) in [
        ("stored_neutral_matte", &migrated),
        ("enabled_but_selecting_everything", &enabled_neutral),
    ] {
        assert!(
            !MatteParams::from_effect(candidate).has_matte(),
            "{label}: CC5 §2.6 makes this matte inactive"
        );
        let cc4_bytes = crate::compositor::grade_buffer_bytes_for(
            std::slice::from_ref(&cc4_era),
            None,
            CC5_RESOLUTION,
            None,
        )
        .expect("the CC4-era stack serializes");
        let cc5_bytes = crate::compositor::grade_buffer_bytes_for(
            std::slice::from_ref(candidate),
            None,
            CC5_RESOLUTION,
            None,
        )
        .expect("the migrated stack serializes");
        assert_eq!(
            cc4_bytes, cc5_bytes,
            "{label}: an inactive matte must produce a byte-identical grade buffer"
        );
        assert_eq!(
            grade_word(&cc5_bytes, GRADE_NODE_MATTE_OFFSET_WORD),
            0.0,
            "{label}: v11 must stay 0, which is what makes the shader take the CC4 path"
        );
        let cc4_linear = cpu_reference_linear(&frame, &cpu_nodes(std::slice::from_ref(&cc4_era)));
        let cc5_linear = cpu_reference_linear(&frame, &cpu_nodes(std::slice::from_ref(candidate)));
        assert_eq!(
            to_bits(&cc4_linear),
            to_bits(&cc5_linear),
            "{label}: the CPU reference must be to_bits-identical"
        );
        assert_eq!(
            gpu_monitor(&compositor, &frame, std::slice::from_ref(&cc4_era), None),
            gpu_monitor(&compositor, &frame, std::slice::from_ref(candidate), None),
            "{label}: the monitor render must be byte-identical"
        );
        assert_eq!(
            to_bits(&gpu_linear(
                &compositor,
                &frame,
                std::slice::from_ref(&cc4_era),
                None
            )),
            to_bits(&gpu_linear(
                &compositor,
                &frame,
                std::slice::from_ref(candidate),
                None
            )),
            "{label}: the GPU working surface must be to_bits-identical"
        );
    }
    // A matte-free node is position-independent, which is the other half of
    // §2.5.4: the reference must not take the blend path at all.
    let nodes = cpu_nodes(std::slice::from_ref(&cc4_era));
    let sample = [0.25_f32, 0.5, 0.75];
    let aspect = CC5_RASTER_WIDTH as f32 / CC5_RASTER_HEIGHT as f32;
    let centre = apply_color_nodes_at(&nodes, sample, [0.5, 0.5], aspect);
    for uv in [[0.0_f32, 0.0], [1.0, 1.0], [0.125, 0.875]] {
        assert_eq!(
            to_bits(&apply_color_nodes_at(&nodes, sample, uv, aspect)),
            to_bits(&centre),
            "a matte-free node must be independent of position"
        );
    }

    // --- the `mask` regression -------------------------------------------
    let mask = color_node_effect(
        9,
        "mask",
        vec![
            // A rect offset to the left, so its edge at u.x = 0.6 falls
            // *inside* the matte window's 0.25..=0.75: all four
            // mask × matte quadrants are then non-empty.
            ("shape_token".to_owned(), 1),
            ("center_x_percent".to_owned(), 30),
            ("center_y_percent".to_owned(), 50),
            ("width_percent".to_owned(), 60),
            ("height_percent".to_owned(), 100),
            ("feather_percent".to_owned(), 0),
            ("invert".to_owned(), 0),
        ],
    );
    let matte = MatteSpec::window(WindowSpec::CENTRED);
    let matted = gain_wheels(1, 1_500, Some(&matte));
    let mask_only = gpu_linear(&compositor, &frame, std::slice::from_ref(&mask), None);
    let matte_only = gpu_linear(&compositor, &frame, std::slice::from_ref(&matted), None);
    let both = gpu_linear(&compositor, &frame, &[mask.clone(), matted.clone()], None);
    let covered = matte.covered_pixels(&raster);
    // `render_working` composites the layer over the cleared, opaque target,
    // so the layer's own alpha is observed as its premultiplied contribution:
    // a masked-out pixel contributes exactly nothing. The §9.1 raster has no
    // zero channel, so "contributed" and "did not contribute" are decidable.
    let mut quadrants = [0_usize; 4];
    for index in 0..CC5_RASTER_PIXELS {
        let inside_mask = mask_only[index * 4] != 0.0;
        assert_eq!(
            inside_mask,
            both[index * 4] != 0.0,
            "pixel {index}: adding a matte changed which pixels the mask admits; the layer alpha \
             must be exactly the mask-only alpha"
        );
        quadrants[usize::from(inside_mask) * 2 + usize::from(covered[index])] += 1;
        for channel in 0..3 {
            let expected = if inside_mask {
                matte_only[index * 4 + channel]
            } else {
                mask_only[index * 4 + channel]
            };
            assert_eq!(
                both[index * 4 + channel].to_bits(),
                expected.to_bits(),
                "pixel {index} channel {channel}: inside the mask the RGB must equal the \
                 matte-only RGB byte for byte, and outside it the mask-only composite"
            );
        }
    }
    for (index, count) in quadrants.iter().enumerate() {
        assert!(
            *count > 0,
            "the mask and the matte must overlap partially; quadrant {index} is empty"
        );
    }
    // And no alpha byte moved anywhere: the composite alpha is opaque in every
    // one of the three renders, mask or no mask, matte or no matte.
    for index in 0..CC5_RASTER_PIXELS {
        for render in [&mask_only, &matte_only, &both] {
            assert_eq!(
                render[index * 4 + 3],
                1.0,
                "CC5 writes no alpha; the composited alpha must stay opaque at pixel {index}"
            );
        }
    }

    emit_cc5_evidence(
        "cc5_migration_and_mask",
        gpu.backend(),
        gpu.lane.id(),
        json!({
            "migration": ["stored_neutral_matte", "enabled_but_selecting_everything"],
            "mask": "rect, centre 30/50, 60% × 100%, feather 0",
            "matte": "rect, centre (5000, 5000), half extents 2500/2500",
        }),
        CC5_RESOLUTION,
        output_hash(&gpu_monitor(&compositor, &frame, &[mask, matted], None)),
        json!({
            "matte_parameter_count": MATTE_PARAMETER_COUNT,
            "mask_outside_matte_outside": quadrants[0],
            "mask_outside_matte_inside": quadrants[1],
            "mask_inside_matte_outside": quadrants[2],
            "mask_inside_matte_inside": quadrants[3],
        }),
    );
}

// ---------------------------------------------------------------------------
// §9.2.9: matte proof fidelity.
// ---------------------------------------------------------------------------

/// The raster as the working surface actually stores it.
///
/// The qualifier reads the value *entering* the node, which is the `f16`
/// working value, not the `f32` the fixture wrote — so a coverage expectation
/// that depends on colour must be computed from the quantized samples.
fn quantized(raster: &[[f32; 3]]) -> Vec<[f32; 3]> {
    raster
        .iter()
        .map(|sample| sample.map(|value| f16::from_f32(value).to_f32()))
        .collect()
}

/// A 64 × 36 source that decodes to a mid grey, tagged BT.709/limited.
///
/// The proof asserts *geometry*, so the picture only has to decode; the raster
/// is the §9.1 raster size so a pixel-exact expectation can be stated.
fn cc5_matte_source(label: &str) -> crate::test_support::GeneratedMedia {
    crate::test_support::GeneratedMedia::ffmpeg(
        label,
        &[
            "-f",
            "lavfi",
            "-i",
            "color=c=gray:size=64x36:rate=25:duration=1",
            "-frames:v",
            "25",
            "-c:v",
            "ffv1",
            "-pix_fmt",
            "yuv444p",
            "-color_primaries",
            "bt709",
            "-color_trc",
            "bt709",
            "-colorspace",
            "bt709",
            "-color_range",
            "tv",
        ],
        "mkv",
    )
}

/// Stage a single-clip 64 × 36 document on a decodable source.
fn cc5_media_document(
    media: &crate::test_support::GeneratedMedia,
    effects: Vec<Effect>,
) -> Arc<Document> {
    let mut asset = crate::decode::probe_path(media.path(), kinewright_core::AssetId(1))
        .expect("the CC5 source should probe");
    assert_eq!(asset.resolution, Some(CC5_RESOLUTION));
    asset.color_description = kinewright_core::ColorDescription {
        primaries: kinewright_core::ColorPrimaries::Bt709,
        transfer: kinewright_core::ColorTransfer::Bt709,
        matrix: kinewright_core::ColorMatrix::Bt709,
        range: kinewright_core::ColorRange::Limited,
        white_point: kinewright_core::ColorWhitePoint::D65,
        bit_depth: kinewright_core::ColorBitDepth::Eight,
        confidence_basis_points: 10_000,
        provenance: kinewright_core::ColorProvenance::UserOverride,
    };
    let mut document = crate::test_support::single_clip_document(asset);
    document.tracks[0].clips[0].effects = effects;
    assert_eq!(document.resolution, CC5_RESOLUTION);
    Arc::new(document)
}

/// CC5 §9.2.9. The rendered coverage equals the CPU reference's
/// `round(255·m)` exactly for every unfeathered case and within one code for a
/// feathered one, the coverage raster is opaque, and a node that carries no
/// matte or is inactive fails typed rather than returning a frame.
#[test]
fn cc5_matte_proof_matches_the_cpu_reference_coverage() {
    let raster = cc5_field_raster();
    let samples = quantized(&raster);
    let frame = frame_of(&raster);
    let gpu = fallback_gpu();
    let compositor = Compositor::new(gpu.context());

    let cases: Vec<(&str, MatteSpec)> = vec![
        ("hard_window", MatteSpec::window(WindowSpec::CENTRED)),
        (
            "feathered_window",
            MatteSpec::window(WindowSpec::CENTRED.with_feather(4_000)),
        ),
        (
            "qualifier_and_feather",
            MatteSpec::window(WindowSpec::CENTRED.with_feather(2_500)).with_qualifier(
                QualifierSpec {
                    luma_low: 2_000,
                    luma_high: 8_000,
                    luma_softness: 1_500,
                    ..QualifierSpec::NEUTRAL
                },
            ),
        ),
        (
            "inverted_with_mix",
            MatteSpec::window(WindowSpec::CENTRED)
                .inverted()
                .with_mix(6_000),
        ),
    ];
    let mut recorded = Vec::new();
    for (label, matte) in &cases {
        let effects = [gain_wheels(1, 1_500, Some(matte))];
        let coverage = gpu_coverage(&compositor, &frame, &effects, EffectId(1));
        let expected = matte.coverage_values(&samples);
        assert_coverage_matches(&coverage, &expected, matte.is_hard_edged(), label);
        let worst = coverage
            .iter()
            .zip(&expected)
            .map(|(actual, expected)| {
                actual.abs_diff((expected.clamp(0.0, 1.0) * 255.0).round() as u8)
            })
            .max()
            .unwrap_or(0);
        recorded.push(json!({
            "case": label,
            "hard_edged": matte.is_hard_edged(),
            "worst_code_difference": worst,
        }));
    }

    // --- typed refusals ---------------------------------------------------
    let matte_free = gain_wheels(1, 1_500, None);
    let excluded = gain_wheels(
        2,
        1_500,
        Some(&MatteSpec::window(WindowSpec::CENTRED).with_mix(0)),
    );
    let not_a_node = color_node_effect(3, "brightness", vec![("percent".to_owned(), 10)]);
    let stack = vec![matte_free.clone(), excluded.clone(), not_a_node.clone()];
    let refusal = |effect: EffectId| -> String {
        compositor
            .render_matte(
                CC5_RESOLUTION,
                &[CompositorLayer {
                    frame: &frame,
                    effects: &stack,
                    transition: TransitionRenderParams::default(),
                }],
                None,
                MatteRenderTarget {
                    layer_index: 0,
                    clip: ClipId(1),
                    effect,
                },
            )
            .expect_err("a proof never returns a blank frame")
            .to_string()
    };
    let no_matte = refusal(EffectId(1));
    assert!(
        no_matte.contains("matte_proof_no_matte"),
        "unexpected refusal: {no_matte}"
    );
    let inactive = refusal(EffectId(2));
    assert!(
        inactive.contains("matte_proof_node_inactive")
            && inactive.contains(ColorNodeInactiveReason::MatteExcluded.as_str()),
        "the refusal must name the inactivity reason: {inactive}"
    );
    let wrong_kind = refusal(EffectId(3));
    assert!(
        wrong_kind.contains("matte_proof_not_a_color_node") && wrong_kind.contains("brightness"),
        "the refusal must name the effect that was found: {wrong_kind}"
    );
    let missing = refusal(EffectId(99));
    assert!(
        missing.contains("matte_proof_effect_not_found") && missing.contains("99"),
        "the refusal must name the effect that was asked for: {missing}"
    );

    // --- the production `Analysis` proof ----------------------------------
    // CC5 §4.1's proof is rendered by the production compositor through
    // `engine.rs`, so the fixture asserts the whole path, including the
    // transfer-free readback and the opaque alpha.
    crate::initialize_ffmpeg().expect("FFmpeg must initialize for the CC5 proof fixture");
    let media = cc5_matte_source("cc5-matte-proof");
    let matte = MatteSpec::window(WindowSpec::CENTRED);
    let document = cc5_media_document(&media, vec![gain_wheels(7, 1_500, Some(&matte))]);
    let engine = crate::engine::FfmpegMediaEngine::new_with_gpu(gpu.context())
        .expect("the media engine should start on the fixture adapter");
    let proof = engine
        .matte_proof_for_document(
            Arc::clone(&document),
            TimeCode::ZERO,
            ClipId(1),
            EffectId(7),
        )
        .expect("the CC5 §4.1 matte proof should render");
    assert_eq!(
        (proof.coverage.width, proof.coverage.height),
        CC5_RESOLUTION
    );
    let covered = hand_derived_centred_window();
    for (index, pixel) in proof.coverage.pixels.as_chunks::<4>().0.iter().enumerate() {
        assert_eq!(pixel[3], u8::MAX, "coverage pixel {index} is not opaque");
        assert_eq!(pixel[0], pixel[1]);
        assert_eq!(pixel[1], pixel[2]);
        assert_eq!(
            pixel[0],
            u8::from(covered[index]) * 255,
            "coverage pixel {index} is not the hand-derived round(255·m)"
        );
    }
    assert_eq!(proof.metadata.node_kind, "color_wheels");
    assert_eq!(
        proof.metadata.coverage_encoding,
        kinewright_core::MATTE_COVERAGE_ENCODING
    );
    assert_eq!(
        proof.metadata.coverage_scale,
        kinewright_core::MATTE_COVERAGE_SCALE
    );
    // round(1e6 · 64 / 36)
    assert_eq!(proof.metadata.raster_aspect_millionths, 1_777_778);
    assert!(proof.metadata.matte_enabled);
    assert_eq!(proof.metadata.window_count, 1);
    assert!(!proof.metadata.qualifier_enabled);
    gpu.assert_proof_provenance(&proof.metadata.render);
    // The statistics Core derives from that raster are the hand-derived ones.
    let statistics = kinewright_core::matte_coverage_statistics(&proof.coverage)
        .expect("the coverage raster is well formed");
    assert_eq!(statistics.covered_pixel_count, 576);
    assert_eq!(statistics.full_pixel_count, 576);
    assert_eq!(statistics.partial_pixel_count, 0);
    assert_eq!(
        statistics.covered_basis_points,
        CENTRED_WINDOW_BASIS_POINTS as u32
    );
    assert!(statistics.weighted_by_coverage);

    emit_cc5_evidence(
        "cc5_matte_proof",
        gpu.backend(),
        gpu.lane.id(),
        json!({
            "cases": cases.iter().map(|(label, _)| *label).collect::<Vec<_>>(),
            "analysis_proof": "color_wheels with a centred 2500/2500 rect window",
        }),
        CC5_RESOLUTION,
        output_hash(&proof.coverage.pixels),
        json!({
            "coverage_cases": recorded,
            "covered_pixel_count": statistics.covered_pixel_count,
            "covered_basis_points": statistics.covered_basis_points,
            "refusals": [no_matte, inactive, wrong_kind, missing],
        }),
    );
}

// ---------------------------------------------------------------------------
// §9.2.17: skin and product qualifier fixtures.
// ---------------------------------------------------------------------------

/// The §9.2.17 chart, in `grade709` encoding: four skin patches from light to
/// deep, then a saturated red and a saturated cyan product patch.
///
/// The selectors are hand-derivable straight from these triples, because the
/// encoding is what the qualifier judges: `S = (M − m)/M`,
/// `Y = 0.2126r + 0.7152g + 0.0722b`, and `H = 60·(g − b)/C` on the red
/// sector. The expected values below are those arithmetic results, not
/// measurements.
const CHART_PATCHES: [(&str, [f32; 3], f64, f64, f64); 6] = [
    ("skin_light", [0.85, 0.68, 0.60], 0.294_118, 0.710_366, 19.2),
    (
        "skin_medium",
        [0.72, 0.53, 0.44],
        0.388_889,
        0.563_896,
        19.285_714,
    ),
    ("skin_tan", [0.55, 0.38, 0.30], 0.454_545, 0.410_366, 19.2),
    (
        "skin_deep",
        [0.32, 0.20, 0.15],
        0.531_250,
        0.221_902,
        17.647_059,
    ),
    (
        "product_red",
        [0.80, 0.10, 0.12],
        0.875_000,
        0.250_264,
        358.285_714,
    ),
    (
        "product_cyan",
        [0.10, 0.65, 0.75],
        0.866_667,
        0.540_290,
        189.230_769,
    ),
];

/// The neutral surround the patches sit in: `C = 0`, so a qualifier that names
/// a hue must exclude it and the `18000` escape must include it.
const CHART_SURROUND: [f32; 3] = [0.45, 0.45, 0.45];

const CHART_PATCH_WIDTH: u32 = 8;
const CHART_PATCH_TOP: u32 = 12;
const CHART_PATCH_HEIGHT: u32 = 12;
const CHART_PATCH_PIXELS: usize = (CHART_PATCH_WIDTH * CHART_PATCH_HEIGHT) as usize;

/// Which chart patch, if any, owns raster pixel `index`.
fn chart_patch_of(index: usize) -> Option<usize> {
    let x = index as u32 % CC5_RASTER_WIDTH;
    let y = index as u32 / CC5_RASTER_WIDTH;
    if !(CHART_PATCH_TOP..CHART_PATCH_TOP + CHART_PATCH_HEIGHT).contains(&y) {
        return None;
    }
    let column = x % 10;
    let patch = (x / 10) as usize;
    (column >= 2 && patch < CHART_PATCHES.len()).then_some(patch)
}

fn cc5_chart_raster() -> Vec<[f32; 3]> {
    (0..CC5_RASTER_PIXELS)
        .map(|index| {
            let encoded =
                chart_patch_of(index).map_or(CHART_SURROUND, |patch| CHART_PATCHES[patch].1);
            qualifier_input(encoded)
        })
        .collect()
}

/// CC5 §9.2.17. A qualifier tuned to one skin band selects every pixel of its
/// patch and exactly zero pixels of the surround, the other skin patches, and
/// the product patches; the same holds for a product-hue qualifier.
///
/// This is the roadmap's named exit evidence for workflow #5 and is **not** a
/// skin-tone quality claim: the qualifier is a deterministic selector, not a
/// perceptual model (CC6 owns skin QC).
#[test]
fn cc5_skin_and_product_qualifiers_select_only_their_patch() {
    let raster = cc5_chart_raster();
    let samples = quantized(&raster);
    let frame = frame_of(&raster);
    let gpu = fallback_gpu();
    let compositor = Compositor::new(gpu.context());

    // --- the chart's selectors are the hand-derived arithmetic ------------
    let mut selectors = Vec::new();
    for (patch, (label, encoded, saturation, luma, hue)) in CHART_PATCHES.iter().enumerate() {
        let index = (0..CC5_RASTER_PIXELS)
            .find(|index| chart_patch_of(*index) == Some(patch))
            .expect("every patch is on the raster");
        let (actual_saturation, actual_luma, actual_hue) = spec_selectors_f64(samples[index]);
        assert!(
            (actual_saturation - saturation).abs() < 2.0e-3,
            "{label}: S = {actual_saturation}, hand-derived {saturation}"
        );
        assert!(
            (actual_luma - luma).abs() < 2.0e-3,
            "{label}: Y = {actual_luma}, hand-derived {luma}"
        );
        assert!(
            (actual_hue.expect("a chromatic patch") - hue).abs() < 0.1,
            "{label}: H = {actual_hue:?}, hand-derived {hue}"
        );
        assert_eq!(
            samples
                .iter()
                .enumerate()
                .filter(|(index, _)| chart_patch_of(*index) == Some(patch))
                .count(),
            CHART_PATCH_PIXELS
        );
        selectors.push(json!({
            "patch": label,
            "encoded": encoded,
            "saturation": actual_saturation,
            "luma": actual_luma,
            "hue": actual_hue,
        }));
    }
    let surround_index = (0..CC5_RASTER_PIXELS)
        .find(|index| chart_patch_of(*index).is_none())
        .expect("the surround exists");
    let (surround_saturation, _, surround_hue) = spec_selectors_f64(samples[surround_index]);
    assert_eq!(surround_saturation, 0.0);
    assert_eq!(surround_hue, None, "the surround is achromatic");

    // --- one skin band and one product hue --------------------------------
    let cases = [
        (
            "skin_tan",
            2_usize,
            QualifierSpec {
                // 19.2° ± 5°, saturation 0.30..0.70, luma 0.38..0.44 — the
                // band that separates `skin_tan` (Y = 0.4104) from
                // `skin_medium` (0.5639) and `skin_deep` (0.2219).
                hue_center: 1_920,
                hue_width: 500,
                sat_low: 3_000,
                sat_high: 7_000,
                luma_low: 3_800,
                luma_high: 4_400,
                ..QualifierSpec::NEUTRAL
            },
        ),
        (
            "product_red",
            4_usize,
            QualifierSpec {
                // 358° ± 3°, saturation 0.80..1.00: the red product only —
                // the cyan is 189° and every skin patch is under 0.54.
                hue_center: 35_800,
                hue_width: 300,
                sat_low: 8_000,
                sat_high: 10_000,
                ..QualifierSpec::NEUTRAL
            },
        ),
    ];
    let baseline_linear = cpu_reference_linear(&frame, &[]);
    let baseline_monitor = cpu_reference_monitor(&frame, &[]);
    let mut recorded = Vec::new();
    for (label, patch, qualifier) in cases {
        let matte = MatteSpec::qualifier(qualifier);
        let covered = matte.covered_pixels(&samples);
        let expected = (0..CC5_RASTER_PIXELS)
            .map(|index| chart_patch_of(index) == Some(patch))
            .collect::<Vec<_>>();
        assert_eq!(
            covered, expected,
            "the {label} qualifier must select exactly its own patch"
        );
        assert_eq!(
            covered.iter().filter(|covered| **covered).count(),
            CHART_PATCH_PIXELS
        );

        let graded = gain_wheels(1, 1_500, Some(&matte));
        let cpu_linear = cpu_reference_linear(&frame, &cpu_nodes(std::slice::from_ref(&graded)));
        let cpu_monitor = cpu_reference_monitor(&frame, &cpu_nodes(std::slice::from_ref(&graded)));
        let counts = assert_matte_containment(&cpu_linear, &baseline_linear, &covered, label);
        assert_eq!(
            counts.inside_changed_pixels, CHART_PATCH_PIXELS,
            "every pixel of the selected patch must change"
        );
        assert_monitor_containment(&cpu_monitor, &baseline_monitor, &covered, label);

        let gpu_baseline = gpu_linear(&compositor, &frame, &[], None);
        let gpu_graded = gpu_linear(&compositor, &frame, std::slice::from_ref(&graded), None);
        assert_matte_containment(
            &gpu_graded,
            &gpu_baseline,
            &covered,
            &format!("{label}_gpu"),
        );
        let coverage = gpu_coverage(
            &compositor,
            &frame,
            std::slice::from_ref(&graded),
            EffectId(1),
        );
        assert_coverage_matches(&coverage, &matte.coverage_values(&samples), true, label);
        recorded.push(json!({
            "case": label,
            "selected_pixels": CHART_PATCH_PIXELS,
            "containment": counts.as_json(),
        }));
    }

    emit_cc5_evidence(
        "cc5_skin_and_product",
        gpu.backend(),
        gpu.lane.id(),
        json!({
            "chart": "four skin patches and two product patches in a neutral surround",
            "patches": CHART_PATCHES.map(|(label, ..)| label),
            "surround": CHART_SURROUND,
        }),
        CC5_RESOLUTION,
        json_hash(&json!(selectors)),
        json!({"selectors": selectors, "cases": recorded}),
    );
}

// ---------------------------------------------------------------------------
// §9.2.10: matte-scoped scopes.
// ---------------------------------------------------------------------------

/// CC5 §9.2.10, media half. The measured population of a matte-scoped scope is
/// exactly the affected set, an ROI intersects it, and `compare_scope_evidence`
/// refuses to difference a scoped result against an unscoped one.
///
/// The `get_video_scopes_v2` request field and `render_color_proof`'s
/// `matte_comparison` variants are the agent crate's half of this item; what
/// media owns is the analysis-only RGBA copy, its counts, and the comparison
/// rule, all measured on the production monitor and matte proofs of one
/// document at one frame.
#[test]
fn cc5_matte_scoped_scopes_measure_exactly_the_affected_set() {
    crate::initialize_ffmpeg().expect("FFmpeg must initialize for the CC5 scope fixture");
    let gpu = fallback_gpu();
    let media = cc5_matte_source("cc5-matte-scope");
    let matte = MatteSpec::window(WindowSpec::CENTRED);
    let document = cc5_media_document(&media, vec![gain_wheels(7, 1_500, Some(&matte))]);
    let engine = crate::engine::FfmpegMediaEngine::new_with_gpu(gpu.context())
        .expect("the media engine should start on the fixture adapter");

    let monitor = engine
        .monitor_proof_for_document(Arc::clone(&document), TimeCode::ZERO)
        .expect("the managed monitor proof should render");
    let coverage = engine
        .matte_proof_for_document(
            Arc::clone(&document),
            TimeCode::ZERO,
            ClipId(1),
            EffectId(7),
        )
        .expect("the matte proof should render");
    assert_eq!(
        (monitor.image.width, monitor.image.height),
        (coverage.coverage.width, coverage.coverage.height),
        "a matte-scoped scope requires both rasters to be the same size"
    );

    // CC5 §4.3: the analysis-only copy sets `A = 255 if m > 0 else 0`. The
    // document, the render, and the layer alpha are never touched.
    let scoped = kinewright_core::matte_scoped_frame(&monitor.image, &coverage.coverage)
        .expect("the coverage raster scopes the monitor frame");
    for (index, pixel) in scoped.pixels.as_chunks::<4>().0.iter().enumerate() {
        let expected = u8::from(hand_derived_centred_window()[index]) * 255;
        assert_eq!(pixel[3], expected, "scoped alpha at pixel {index}");
        assert_eq!(
            &pixel[..3],
            &monitor.image.pixels[index * 4..index * 4 + 3],
            "the analysis copy must not touch colour"
        );
    }

    let request = kinewright_core::ScopeRequest::default();
    let evidence = kinewright_core::measure_scope(&scoped, 0, &request)
        .expect("the matte-scoped frame measures");
    assert_eq!(
        evidence.metadata.transparent_pixel_count, CENTRED_WINDOW_OUTSIDE_PIXELS as u64,
        "the pixels outside the matte are exactly the transparent ones"
    );
    assert_eq!(
        evidence.metadata.visible_pixel_count, CENTRED_WINDOW_PIXELS as u64,
        "the measured population is exactly the affected set"
    );
    assert_eq!(evidence.metadata.roi_pixel_count, CC5_RASTER_PIXELS as u64);

    // --- ROI ∩ matte ------------------------------------------------------
    // The left half is columns 0..=31; the window covers 16..=47, so the
    // intersection is columns 16..=31 across rows 9..=26: 16 × 18 = 288.
    let half = kinewright_core::ScopeRequest {
        roi: kinewright_core::NormalizedRoi::new(0, 0, 5_000, 10_000),
        ..kinewright_core::ScopeRequest::default()
    };
    let intersected =
        kinewright_core::measure_scope(&scoped, 0, &half).expect("the half-frame ROI measures");
    assert_eq!(intersected.metadata.roi_pixel_count, 32 * 36);
    assert_eq!(intersected.metadata.visible_pixel_count, 288);
    assert_eq!(intersected.metadata.transparent_pixel_count, 32 * 36 - 288);

    // --- a scoped result is not comparable against an unscoped one --------
    let unscoped = kinewright_core::measure_scope(&monitor.image, 0, &request)
        .expect("the unscoped frame measures");
    let mut declared = evidence.clone();
    declared.metadata.matte_region = Some(kinewright_core::MatteRegionDescription::new(
        ClipId(1),
        EffectId(7),
        evidence.metadata.visible_pixel_count,
    ));
    let error = kinewright_core::compare_scope_evidence(&unscoped, &declared)
        .expect_err("a matte-scoped measurement covers a different population");
    match &error {
        kinewright_core::ScopeComparisonError::MatteRegionMismatch {
            reference,
            candidate,
        } => {
            assert!(reference.is_none(), "the unscoped side declares no region");
            let candidate = candidate.as_ref().expect("the scoped side declares one");
            assert_eq!(candidate.clip, ClipId(1));
            assert_eq!(candidate.effect, EffectId(7));
            assert_eq!(candidate.threshold, kinewright_core::MATTE_SCOPE_THRESHOLD);
            assert_eq!(candidate.covered_pixel_count, CENTRED_WINDOW_PIXELS as u64);
        }
        other => panic!("unexpected comparison error: {other:?}"),
    }
    // Two identically scoped measurements still compare.
    kinewright_core::compare_scope_evidence(&declared, &declared)
        .expect("two identically scoped measurements are comparable");

    emit_cc5_evidence(
        "cc5_matte_scoped_scopes",
        gpu.backend(),
        gpu.lane.id(),
        json!({
            "matte": "rect, centre (5000, 5000), half extents 2500/2500",
            "threshold": kinewright_core::MATTE_SCOPE_THRESHOLD,
            "roi": "full frame, and the left half",
        }),
        CC5_RESOLUTION,
        output_hash(&scoped.pixels),
        json!({
            "visible_pixel_count": evidence.metadata.visible_pixel_count,
            "transparent_pixel_count": evidence.metadata.transparent_pixel_count,
            "roi_visible_pixel_count": intersected.metadata.visible_pixel_count,
            "roi_pixel_count": intersected.metadata.roi_pixel_count,
            "comparison_rejection": format!("{error}"),
        }),
    );
}

// ---------------------------------------------------------------------------
// §9.2.16: performance evidence.
// ---------------------------------------------------------------------------

/// The §9.2.16 raster: a full 1920 × 1080 frame, so the recorded time is the
/// per-frame cost a colourist would actually pay.
const PERFORMANCE_RESOLUTION: (u32, u32) = (1920, 1080);
/// Timed renders after the warm-up. The minimum is reported alongside the
/// mean, because a first-run shader compile or an allocator hiccup is not the
/// per-frame cost this evidence is about.
const PERFORMANCE_SAMPLES: usize = 3;

/// The §9.2.16 worst-case stack: sixteen nodes, each carrying four windows and
/// an enabled qualifier.
fn performance_stack() -> Vec<Effect> {
    let matte = MatteSpec::window(WindowSpec::CENTRED)
        .with_windows(
            vec![
                WindowSpec::CENTRED
                    .with_centre(3_000, 3_000)
                    .with_feather(1_500),
                WindowSpec::CENTRED
                    .with_centre(7_000, 3_000)
                    .with_rotation(3_000),
                WindowSpec::CENTRED
                    .with_centre(3_000, 7_000)
                    .with_shape(SHAPE_ELLIPSE),
                WindowSpec::CENTRED.with_centre(7_000, 7_000).inverted(),
            ],
            COMBINE_UNION,
        )
        .with_qualifier(QualifierSpec {
            hue_center: 1_920,
            hue_width: 6_000,
            hue_softness: 2_000,
            sat_low: 500,
            sat_high: 9_500,
            sat_softness: 1_000,
            luma_low: 500,
            luma_high: 9_500,
            luma_softness: 1_000,
        });
    (0..kinewright_core::COLOR_NODE_LIMIT_PER_LAYER)
        .map(|index| {
            curves_effect(
                index as u64 + 1,
                &[(0, 0), (2_500, 1_800), (7_500, 8_200), (10_000, 10_000)],
                Some(&matte),
            )
        })
        .collect()
}

/// Measure and record the §9.2.16 render time on one lane.
///
/// Recorded evidence, **not** a gate: the software rasterizer is orders of
/// magnitude slower than any adapter a colourist uses, and a timing assertion
/// on CI would measure the runner rather than the shader. The soft budget is
/// stated in the payload so a regression is visible.
fn record_cc5_performance(gpu: &FixtureGpu) {
    let compositor = Compositor::new(gpu.context());
    let (width, height) = PERFORMANCE_RESOLUTION;
    let raster = (0..(width * height) as usize)
        .map(|index| {
            let x = (index as u32 % width) as f32 / width as f32;
            let y = (index as u32 / width) as f32 / height as f32;
            [0.05 + 0.9 * x, 0.05 + 0.9 * y, 0.05 + 0.45 * (x + y)]
        })
        .collect::<Vec<_>>();
    let frame = working_frame(width, height, &raster);
    let stack = performance_stack();
    let render = |effects: &[Effect]| {
        compositor
            .render_monitor_with_luts(
                PERFORMANCE_RESOLUTION,
                &[CompositorLayer {
                    frame: &frame,
                    effects,
                    transition: TransitionRenderParams::default(),
                }],
                &kinewright_core::ColorContext::sdr_rec709().monitoring,
                None,
            )
            .expect("the performance stack renders")
    };
    // Warm up: the first render compiles the pipeline and allocates the pool.
    let warm = render(&stack);
    assert_eq!(warm.rgba.len(), (width * height * 4) as usize);
    let mut milliseconds = Vec::with_capacity(PERFORMANCE_SAMPLES);
    for _ in 0..PERFORMANCE_SAMPLES {
        let started = Instant::now();
        let output = render(&stack);
        milliseconds.push(started.elapsed().as_secs_f64() * 1_000.0);
        // Consume the output so the render cannot be optimized away.
        assert_eq!(output.rgba.len(), (width * height * 4) as usize);
    }
    // An empty stack at the same resolution, so the readback and mapping cost
    // the timing unavoidably includes can be subtracted rather than mistaken
    // for shader time. A GPU timestamp query is not available through this
    // path, so the difference is the honest statement of node-stack cost.
    let _ = render(&[]);
    let mut empty_samples = Vec::with_capacity(PERFORMANCE_SAMPLES);
    for _ in 0..PERFORMANCE_SAMPLES {
        let started = Instant::now();
        let output = render(&[]);
        empty_samples.push(started.elapsed().as_secs_f64() * 1_000.0);
        assert_eq!(output.rgba.len(), (width * height * 4) as usize);
    }
    let empty_milliseconds = empty_samples.iter().copied().fold(f64::INFINITY, f64::min);
    // The matte-free cost of the same sixteen nodes, so the matte's share of
    // the frame is visible rather than inferred.
    let matte_free = (0..kinewright_core::COLOR_NODE_LIMIT_PER_LAYER)
        .map(|index| {
            curves_effect(
                index as u64 + 1,
                &[(0, 0), (2_500, 1_800), (7_500, 8_200), (10_000, 10_000)],
                None,
            )
        })
        .collect::<Vec<_>>();
    let _ = render(&matte_free);
    let baseline_started = Instant::now();
    let _ = render(&matte_free);
    let baseline_milliseconds = baseline_started.elapsed().as_secs_f64() * 1_000.0;

    let minimum = milliseconds.iter().copied().fold(f64::INFINITY, f64::min);
    let mean = milliseconds.iter().sum::<f64>() / milliseconds.len() as f64;
    let node_stack_milliseconds = minimum - empty_milliseconds;
    println!(
        "CC5_PERFORMANCE lane={} minimum_ms={minimum:.3} mean_ms={mean:.3} \
         empty_stack_ms={empty_milliseconds:.3} node_stack_ms={node_stack_milliseconds:.3} \
         matte_free_ms={baseline_milliseconds:.3} soft_budget_ms={PERFORMANCE_SOFT_BUDGET_MILLISECONDS}",
        gpu.lane.id()
    );
    let measurements = json!({
            "minimum_milliseconds": minimum,
            "mean_milliseconds": mean,
            "samples_milliseconds": milliseconds,
            "matte_free_milliseconds": baseline_milliseconds,
            "empty_stack_milliseconds": empty_milliseconds,
            "empty_stack_samples_milliseconds": empty_samples,
            "node_stack_milliseconds": node_stack_milliseconds,
            "readback_note": "every render path reads the frame back and converts it on the CPU, so the empty-stack time is the floor this measurement cannot avoid; node_stack_milliseconds is the difference",
            "soft_budget_met_by_node_stack": node_stack_milliseconds <= PERFORMANCE_SOFT_BUDGET_MILLISECONDS,
            "soft_budget_milliseconds": PERFORMANCE_SOFT_BUDGET_MILLISECONDS,
            "soft_budget_met": minimum <= PERFORMANCE_SOFT_BUDGET_MILLISECONDS,
            "gate": "recorded evidence only; CC5 §9.2.16 states a soft budget on the hardware lane",
        "note": "the two node timings are not ordered a priori: CC5 §2.5.5's exact-zero early-out skips a matte-carrying node entirely wherever the coverage is 0, so the matte stack can cost less than the same nodes without mattes",
    });
    // §9.2.16 is recorded evidence, so the field names are part of the
    // contract: a renamed key breaks whatever reads the artefact to see a
    // regression, silently. The manifest declares the same list.
    assert_eq!(
        measurements
            .as_object()
            .expect("the §9.2.16 measurements are an object")
            .keys()
            .cloned()
            .collect::<std::collections::BTreeSet<_>>(),
        PERFORMANCE_EVIDENCE_FIELDS
            .iter()
            .map(|field| (*field).to_owned())
            .collect::<std::collections::BTreeSet<_>>(),
        "the §9.2.16 evidence payload does not carry the declared measurement fields"
    );
    emit_cc5_evidence(
        "cc5_performance",
        gpu.backend(),
        gpu.lane.id(),
        json!({
            "stack": "sixteen color_curves nodes, each with four windows and an enabled qualifier",
            "resolution": [width, height],
            "samples": PERFORMANCE_SAMPLES,
        }),
        PERFORMANCE_RESOLUTION,
        output_hash(&warm.rgba),
        measurements,
    );
}

/// CC5 §9.2.16 on the default software lane.
#[test]
fn cc5_performance_evidence_is_recorded_on_software_fallback() {
    record_cc5_performance(&fallback_gpu());
}

/// CC5 §9.2.16 on a physical adapter, where the soft budget applies.
#[test]
#[ignore = "requires a physical supported GPU; run explicitly with --ignored --nocapture"]
fn cc5_performance_evidence_is_recorded_on_hardware() {
    record_cc5_performance(&hardware_gpu());
}

// ---------------------------------------------------------------------------
// §9.2.11: the tracked shot.
//
// CC5 §9.2.11 is split by crate. Media owns the generated clip, the analytic
// box, the keyframed window, and the CPU/GPU containment of that window at
// every frame; the `track_matte_window` tool — its observations, its smoothed
// curves, its tolerances, and its prepared plan — is the agent crate's half,
// and the manifest records that owner explicitly.
// ---------------------------------------------------------------------------

const TRACK_WIDTH: u32 = 640;
const TRACK_HEIGHT: u32 = 360;
const TRACK_FRAMES: i64 = 100;
const TRACK_FPS: i64 = 25;
/// The white subject is 80 × 80 pixels.
const TRACK_BOX: i64 = 80;
/// `step_frames = 5` over `0..100` yields even intervals, not multiples of 5.
const TRACK_STEP_FRAMES: i64 = 5;
/// The §9.2.11 window half-extents, in basis points of width and of height.
const TRACK_HALF_WIDTH_BASIS_POINTS: i64 = 1_300;
const TRACK_HALF_HEIGHT_BASIS_POINTS: i64 = 1_800;

/// The `tracking_sample_frames(0..100, 5)` sequence: `0, 4, 9, …, 94, 99`.
fn tracking_sample_frames() -> Vec<i64> {
    let mut frames = vec![0];
    let mut frame = TRACK_STEP_FRAMES - 1;
    while frame < TRACK_FRAMES {
        frames.push(frame);
        frame += TRACK_STEP_FRAMES;
    }
    frames
}

/// The analytic top-left corner of the realised box at clip-local `frame`.
///
/// `overlay` exposes `t` as time and evaluates its expressions per frame:
/// `x = 320 + 120·sin(2πt/8) − 40`, `y = 180 + 60·sin(2πt/8) − 40`. The
/// realised box snaps to even pixel offsets because the muxed stream is
/// `yuv420p`, so the expectation is `2·floor(edge/2)`.
fn analytic_box_corner(frame: i64) -> (i64, i64) {
    let t = frame as f64 / TRACK_FPS as f64;
    let phase = (2.0 * std::f64::consts::PI * t / 8.0).sin();
    let x = 320.0 + 120.0 * phase - 40.0;
    let y = 180.0 + 60.0 * phase - 40.0;
    let snap = |value: f64| 2 * (value / 2.0).floor() as i64;
    (snap(x), snap(y))
}

/// The window centre, in basis points, that puts the analytic box in the
/// middle of the window at `frame`.
fn analytic_centre_basis_points(frame: i64) -> (i64, i64) {
    let (x, y) = analytic_box_corner(frame);
    let centre = |edge: i64, extent: i64| {
        ((edge + TRACK_BOX / 2) as f64 * 10_000.0 / extent as f64).round() as i64
    };
    (
        centre(x, i64::from(TRACK_WIDTH)),
        centre(y, i64::from(TRACK_HEIGHT)),
    )
}

/// The tracking source: a solid dark background with an 80 × 80 white subject
/// on an analytic sinusoidal path, muxed with explicit BT.709 / `tv` tags.
///
/// `drawbox` is deliberately **not** used: the pinned `FFmpeg` 8 filter exposes
/// no time variable and reads a `t` in its expressions as the thickness
/// sentinel, so the box silently never appears. The background is solid on
/// purpose: §5.2's box rule makes the SAD template window-sized, and a static
/// high-contrast background is what pins the match at zero displacement.
fn tracking_source(label: &str) -> crate::test_support::GeneratedMedia {
    crate::test_support::GeneratedMedia::ffmpeg(
        label,
        &[
            "-f",
            "lavfi",
            "-i",
            "color=c=0x303030:s=640x360:r=25",
            "-f",
            "lavfi",
            "-i",
            "color=c=white:s=80x80:r=25",
            "-filter_complex",
            "[0:v][1:v]overlay=x='320+120*sin(2*PI*t/8)-40':y='180+60*sin(2*PI*t/8)-40'",
            "-frames:v",
            "100",
            "-c:v",
            "libx264",
            "-pix_fmt",
            "yuv420p",
            "-color_primaries",
            "bt709",
            "-color_trc",
            "bt709",
            "-colorspace",
            "bt709",
            "-color_range",
            "tv",
            "-x264-params",
            "colorprim=bt709:transfer=bt709:colormatrix=bt709:range=tv",
        ],
        "mp4",
    )
}

/// Decode the whole tracked shot through the managed decoder.
fn decode_tracking_frames(
    path: &std::path::Path,
    description: &kinewright_core::ColorDescription,
) -> Vec<WorkingFrame> {
    let mut decoder = crate::decode::VideoDecoder::open_scaled_managed(
        path,
        kinewright_core::Rational::new(TRACK_FPS as u32, 1).expect("25 fps"),
        None,
        description,
        Some(crate::color_pipeline::ColorSourceProfileAssumption::D65),
    )
    .unwrap_or_else(|error| panic!("managed decode failed for {}: {error}", path.display()));
    let mut cache = crate::cache::FrameCache::<WorkingFrame>::new(TRACK_FRAMES as usize + 8);
    decoder
        .decode_window(TimeCode::ZERO, TimeCode(TRACK_FRAMES - 1), &mut cache)
        .expect("the tracked shot decodes");
    (0..TRACK_FRAMES)
        .map(|frame| {
            cache
                .frame_at_or_before(TimeCode(frame))
                .unwrap_or_else(|| panic!("frame {frame} should be cached"))
        })
        .collect()
}

/// The bounding box of the decoded white subject: linear luma above 0.5.
///
/// The background decodes to roughly 0.03 in linear light and the subject to
/// roughly 1.0, so the threshold is three orders of magnitude away from either.
fn subject_bounding_box(frame: &WorkingFrame) -> (i64, i64, i64, i64) {
    let mut bounds: Option<(i64, i64, i64, i64)> = None;
    for (index, pixel) in frame.pixels.as_chunks::<4>().0.iter().enumerate() {
        if pixel[1].to_f32() <= 0.5 {
            continue;
        }
        let x = i64::from(index as u32 % frame.width);
        let y = i64::from(index as u32 / frame.width);
        bounds = Some(match bounds {
            None => (x, x, y, y),
            Some((left, right, top, bottom)) => {
                (left.min(x), right.max(x), top.min(y), bottom.max(y))
            }
        });
    }
    bounds.expect("the subject is visible in every frame")
}

/// The measured containment evidence of one tracked-window run.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct TrackedWindowContainment {
    /// The smallest distance, in basis points of the frame width, from the
    /// subject box's outermost pixel centre to the window's x edge.
    pub(crate) worst_margin_x_basis_points: f64,
    /// The same in basis points of the frame height.
    pub(crate) worst_margin_y_basis_points: f64,
    /// The frame at which the x margin was worst.
    pub(crate) worst_frame: i64,
    /// Frames at which containment was asserted.
    pub(crate) frames_asserted: i64,
    /// Subject pixels at which `m > 0` was asserted.
    pub(crate) pixels_asserted: u64,
}

/// Assert that a window keyframed to `centres` at `sample_frames` contains
/// **every** pixel of the analytic subject box at **every** frame `0..100`,
/// and return the measured worst margins.
///
/// The centre list is a parameter rather than something this function derives,
/// because the gate has to be runnable against three different curves: the
/// analytic ground truth, the simulated lagged curve below, and — in the agent
/// crate, which owns the `track_matte_window` half of CC5 §9.2.11 — the real
/// smoothed curve the tool emits. A gate that can only be fed ground truth
/// never exercises the lag its margins are budgeted for.
///
/// The keyframes are written straight onto the effect rather than through
/// `SetEffectKeyframes` so a *simulated* curve cannot be mistaken for a
/// validated project edit; the caller asserts the operation accepts the real
/// curve separately.
pub(crate) fn assert_tracked_window_contains_the_subject(
    centres: &[[i64; 2]],
    sample_frames: &[i64],
    interpolation: KeyframeInterpolation,
    half_width_basis_points: i64,
    half_height_basis_points: i64,
    label: &str,
) -> TrackedWindowContainment {
    assert_eq!(
        centres.len(),
        sample_frames.len(),
        "{label}: one centre per sample frame"
    );
    let window = WindowSpec {
        shape: SHAPE_RECT,
        cx: 5_000,
        cy: 5_000,
        hw: half_width_basis_points,
        hh: half_height_basis_points,
        rotation: 0,
        feather: 0,
        invert: 0,
    };
    let mut effect = gain_wheels(1, 1_500, Some(&MatteSpec::window(window)));
    for (axis, name) in [
        (0_usize, "matte_window0_center_x_basis_points"),
        (1, "matte_window0_center_y_basis_points"),
    ] {
        effect.keyframes.insert(
            name.to_owned(),
            AutomationCurve {
                keyframes: sample_frames
                    .iter()
                    .zip(centres)
                    .map(|(frame, centre)| Keyframe {
                        at: TimeCode(*frame),
                        value: centre[axis],
                        interpolation,
                    })
                    .collect(),
            },
        );
    }

    let aspect = TRACK_WIDTH as f32 / TRACK_HEIGHT as f32;
    let mut worst_margin_x = f64::INFINITY;
    let mut worst_margin_y = f64::INFINITY;
    let mut worst_frame = 0_i64;
    let mut pixels_asserted = 0_u64;
    for frame in 0..TRACK_FRAMES {
        let evaluated = effect.evaluated_at(TimeCode(frame));
        let resolved = Matte::from_params(&MatteParams::from_effect(&evaluated))
            .unwrap_or_else(|| panic!("{label}: the tracked matte is active at frame {frame}"));
        let centre_x = evaluated
            .integer_parameter_at("matte_window0_center_x_basis_points", TimeCode(frame))
            .expect("a keyframed centre");
        let centre_y = evaluated
            .integer_parameter_at("matte_window0_center_y_basis_points", TimeCode(frame))
            .expect("a keyframed centre");
        let (left, top) = analytic_box_corner(frame);
        // Every pixel of the box, not only its corners: the window is convex
        // and axis aligned, but the fixture asserts the set rather than the
        // argument.
        for y in top..top + TRACK_BOX {
            for x in left..left + TRACK_BOX {
                let uv = [
                    (x as f32 + 0.5) / TRACK_WIDTH as f32,
                    (y as f32 + 0.5) / TRACK_HEIGHT as f32,
                ];
                assert!(
                    resolved.coverage(uv, aspect, [0.5, 0.5, 0.5]) > 0.0,
                    "{label}, frame {frame}: box pixel ({x}, {y}) is outside the tracked window"
                );
                pixels_asserted += 1;
            }
        }
        // The derived margin, in basis points, from the analytic geometry.
        let margin_x = half_width_basis_points as f64
            - ((left + TRACK_BOX - 1) as f64 + 0.5) * 10_000.0 / f64::from(TRACK_WIDTH)
            + centre_x as f64;
        let margin_x = margin_x.min(
            half_width_basis_points as f64
                + (left as f64 + 0.5) * 10_000.0 / f64::from(TRACK_WIDTH)
                - centre_x as f64,
        );
        let margin_y = half_height_basis_points as f64
            - ((top + TRACK_BOX - 1) as f64 + 0.5) * 10_000.0 / f64::from(TRACK_HEIGHT)
            + centre_y as f64;
        let margin_y = margin_y.min(
            half_height_basis_points as f64
                + (top as f64 + 0.5) * 10_000.0 / f64::from(TRACK_HEIGHT)
                - centre_y as f64,
        );
        if margin_x < worst_margin_x {
            worst_margin_x = margin_x;
            worst_frame = frame;
        }
        worst_margin_y = worst_margin_y.min(margin_y);
    }
    assert!(
        worst_margin_x > 0.0 && worst_margin_y > 0.0,
        "{label}: the measured margins must be positive: {worst_margin_x} / {worst_margin_y} bp"
    );
    TrackedWindowContainment {
        worst_margin_x_basis_points: worst_margin_x,
        worst_margin_y_basis_points: worst_margin_y,
        worst_frame,
        frames_asserted: TRACK_FRAMES,
        pixels_asserted,
    }
}

/// The deterministic raw jitter the §9.2.11 lag simulation adds to each
/// analytic centre, in basis points of the frame extent.
///
/// `MATTE_TRACK_TOLERANCE_BASIS_POINTS = 200` is the contract's gate on the
/// **raw** observations, so ±200 bp is the largest error a passing tracker run
/// may carry. Feeding exactly that worst case into the smoother is what turns
/// the margin budget into a measurement instead of a restatement of ground
/// truth. Deterministic on purpose: a fixture that fails one run in fifty is
/// not evidence.
fn tracking_raw_jitter_basis_points(sample_index: usize, axis: usize) -> i64 {
    let seed = (sample_index as i64 * 2 + axis as i64).wrapping_mul(2_654_435_761);
    seed.rem_euclid(401) - 200
}

/// The measured worst containment margin of the ground-truth curve: the
/// analytic centres keyframed at the tracker's own sample frames, whose only
/// error is the linear interpolation between samples. Worst at frame 50.
const TRACK_ANALYTIC_WORST_MARGIN_X_BASIS_POINTS: f64 = 651.812_5;
const TRACK_ANALYTIC_WORST_MARGIN_Y_BASIS_POINTS: f64 = 647.111_111_111_111_3;
/// The measured worst containment margin of the **simulated lagged** curve:
/// the analytic centres plus the ±200 bp raw tolerance, put through core's
/// `stabilize_tracked_centres_basis_points` with the tool's own constants.
/// Worst at frame 99, which is the last-sample median substitution CC5 §5.2
/// names as the known systematic lag.
const TRACK_LAGGED_WORST_MARGIN_X_BASIS_POINTS: f64 = 192.062_5;
const TRACK_LAGGED_WORST_MARGIN_Y_BASIS_POINTS: f64 = 426.777_777_777_777_4;
/// The measured worst `|smoothed − analytic|` at a sample frame: the lag the
/// smoother introduces on top of the ±200 bp raw error it is rejecting.
const TRACK_SIMULATED_LAG_X_BASIS_POINTS: i64 = 491;
const TRACK_SIMULATED_LAG_Y_BASIS_POINTS: i64 = 276;

/// CC5 §9.2.11, media half. The generated clip really carries the analytic
/// box, and a window keyframed to the analytic centres at the tracker's own
/// sample frames contains every pixel of that box at **every** frame, on the
/// CPU reference and on the GPU, at two layer scales.
#[test]
fn cc5_tracked_shot_window_contains_the_subject_at_every_frame() {
    crate::initialize_ffmpeg().expect("FFmpeg must initialize for the CC5 tracking fixture");
    let media = tracking_source("cc5-tracked-shot");
    let mut asset = crate::decode::probe_path(media.path(), kinewright_core::AssetId(1))
        .expect("the tracked shot should probe");
    assert_eq!(asset.resolution, Some((TRACK_WIDTH, TRACK_HEIGHT)));
    // CC1 rejects an untagged source; the mux states BT.709 / limited
    // explicitly, and the probe must report it rather than assume it.
    assert_eq!(
        asset.color_description.primaries,
        kinewright_core::ColorPrimaries::Bt709
    );
    assert_eq!(
        asset.color_description.transfer,
        kinewright_core::ColorTransfer::Bt709
    );
    assert_eq!(
        asset.color_description.matrix,
        kinewright_core::ColorMatrix::Bt709
    );
    assert_eq!(
        asset.color_description.range,
        kinewright_core::ColorRange::Limited
    );
    asset.duration = TimeCode(TRACK_FRAMES);
    let description = asset.color_description.clone();
    let frames = decode_tracking_frames(media.path(), &description);
    assert_eq!(frames.len() as i64, TRACK_FRAMES);

    // --- the realised box is the even-snapped analytic box ----------------
    let samples = tracking_sample_frames();
    assert_eq!(samples.len(), 21);
    assert_eq!(samples[0], 0);
    assert_eq!(samples[1], 4);
    assert_eq!(samples[2], 9);
    assert_eq!(*samples.last().expect("samples"), 99);
    for pair in samples[1..].windows(2) {
        assert_eq!(
            pair[1] - pair[0],
            TRACK_STEP_FRAMES,
            "tracking_sample_frames must step evenly"
        );
    }
    let mut measured_boxes = Vec::new();
    for frame in [0_i64, 25, 50, 75, 99] {
        let (left, right, top, bottom) = subject_bounding_box(&frames[frame as usize]);
        let (x, y) = analytic_box_corner(frame);
        assert_eq!(
            (left, right, top, bottom),
            (x, x + TRACK_BOX - 1, y, y + TRACK_BOX - 1),
            "frame {frame}: the realised box must land on the even-snapped analytic edges"
        );
        measured_boxes.push(json!({
            "frame": frame,
            "corner": [x, y],
            "bounding_box": [left, right, top, bottom],
        }));
    }

    // --- the keyframed window ---------------------------------------------
    let window = WindowSpec {
        shape: SHAPE_RECT,
        cx: 5_000,
        cy: 5_000,
        hw: TRACK_HALF_WIDTH_BASIS_POINTS,
        hh: TRACK_HALF_HEIGHT_BASIS_POINTS,
        rotation: 0,
        feather: 0,
        invert: 0,
    };
    let matte = MatteSpec::window(window);
    let mut document = crate::test_support::single_clip_document(asset);
    document.tracks[0].clips[0].effects = vec![gain_wheels(1, 1_500, Some(&matte))];
    for (name, axis) in [
        ("matte_window0_center_x_basis_points", 0_usize),
        ("matte_window0_center_y_basis_points", 1),
    ] {
        let keyframes = samples
            .iter()
            .map(|frame| {
                let centre = analytic_centre_basis_points(*frame);
                Keyframe {
                    at: TimeCode(*frame),
                    value: if axis == 0 { centre.0 } else { centre.1 },
                    interpolation: KeyframeInterpolation::Linear,
                }
            })
            .collect::<Vec<_>>();
        kinewright_core::Operation::SetEffectKeyframes {
            clip: ClipId(1),
            effect: EffectId(1),
            name: name.to_owned(),
            curve: AutomationCurve { keyframes },
        }
        .apply(&mut document)
        .expect("the tracked centres are ordinary linear keyframes");
    }

    // --- containment at every frame, on the CPU reference ------------------
    //
    // Run twice, against two different curves, because the margins §9.2.11
    // budgets are budgets for *tracker lag* and a run against ground truth
    // never spends any of them.
    let analytic_centres = samples
        .iter()
        .map(|frame| {
            let (x, y) = analytic_centre_basis_points(*frame);
            [x, y]
        })
        .collect::<Vec<_>>();
    let analytic = assert_tracked_window_contains_the_subject(
        &analytic_centres,
        &samples,
        KeyframeInterpolation::Linear,
        TRACK_HALF_WIDTH_BASIS_POINTS,
        TRACK_HALF_HEIGHT_BASIS_POINTS,
        "analytic_ground_truth",
    );
    assert_eq!(analytic.frames_asserted, TRACK_FRAMES);
    assert_eq!(
        analytic.pixels_asserted,
        (TRACK_FRAMES * TRACK_BOX * TRACK_BOX) as u64
    );
    assert_eq!(analytic.worst_frame, 50);
    // The *measured* worst margin, against a literal. The previous form of
    // this assertion compared `1300 − 625` with `675`, which is constant
    // arithmetic that cannot fail.
    assert!(
        (analytic.worst_margin_x_basis_points - TRACK_ANALYTIC_WORST_MARGIN_X_BASIS_POINTS).abs()
            <= 1.0e-6,
        "the measured ground-truth x margin is {} bp, not the recorded {} bp",
        analytic.worst_margin_x_basis_points,
        TRACK_ANALYTIC_WORST_MARGIN_X_BASIS_POINTS
    );
    assert!(
        (analytic.worst_margin_y_basis_points - TRACK_ANALYTIC_WORST_MARGIN_Y_BASIS_POINTS).abs()
            <= 1.0e-6,
        "the measured ground-truth y margin is {} bp, not the recorded {} bp",
        analytic.worst_margin_y_basis_points,
        TRACK_ANALYTIC_WORST_MARGIN_Y_BASIS_POINTS
    );

    // --- the simulated lagged curve ---------------------------------------
    //
    // The tracker does not observe ground truth. CC5 §9.2.11 budgets
    // `1300 − 625 = 675` bp and `1800 − 1111 = 689` bp of margin against
    // tracker error, and §5.2 states a *known systematic lag*: the median
    // filter replaces the final sample with `median(o[n−3], o[n−2], o[n−1])`,
    // so the last smoothed value lags a moving subject. This leg reproduces
    // both: the analytic centres are perturbed by the contract's own raw
    // tolerance (±200 bp) and put through core's
    // `stabilize_tracked_centres_basis_points` with the tool's pinned
    // constants — the same call `track_matte_window` makes — and the
    // containment gate is run against the result.
    const MATTE_TRACK_DEAD_ZONE_BASIS_POINTS: i64 = 0;
    const MATTE_TRACK_MAX_STEP_BASIS_POINTS: i64 = 800;
    let raw_centres = analytic_centres
        .iter()
        .enumerate()
        .map(|(index, centre)| {
            [
                centre[0] + tracking_raw_jitter_basis_points(index, 0),
                centre[1] + tracking_raw_jitter_basis_points(index, 1),
            ]
        })
        .collect::<Vec<_>>();
    let worst_raw_jitter = raw_centres
        .iter()
        .zip(&analytic_centres)
        .flat_map(|(raw, truth)| [(raw[0] - truth[0]).abs(), (raw[1] - truth[1]).abs()])
        .max()
        .expect("at least one sample");
    assert!(
        worst_raw_jitter <= 200,
        "the simulated raw error must stay inside the contract's 200 bp raw tolerance, not          {worst_raw_jitter} bp"
    );
    assert!(
        worst_raw_jitter >= 190,
        "a simulation that never reaches the raw tolerance does not exercise the budget; the          worst simulated raw error is only {worst_raw_jitter} bp"
    );
    let smoothed_axes = [0_usize, 1].map(|axis| {
        kinewright_core::stabilize_tracked_centres_basis_points(
            &raw_centres
                .iter()
                .map(|centre| centre[axis])
                .collect::<Vec<_>>(),
            kinewright_core::MATTE_WINDOW_CENTER_MIN_BASIS_POINTS,
            kinewright_core::MATTE_WINDOW_CENTER_MAX_BASIS_POINTS,
            MATTE_TRACK_DEAD_ZONE_BASIS_POINTS,
            MATTE_TRACK_MAX_STEP_BASIS_POINTS,
        )
    });
    let smoothed_centres = (0..samples.len())
        .map(|index| [smoothed_axes[0][index], smoothed_axes[1][index]])
        .collect::<Vec<_>>();
    // The lag the smoother costs, measured at the sample frames. §5.2 says the
    // last sample is the worst case, and it is: both axes peak at sample 20,
    // the median substitution at the end of the run.
    let mut lag = [0_i64, 0];
    let mut worst_lag_sample = [0_usize, 0];
    for (index, (smoothed, truth)) in smoothed_centres.iter().zip(&analytic_centres).enumerate() {
        for axis in 0..2 {
            let error = (smoothed[axis] - truth[axis]).abs();
            if error > lag[axis] {
                lag[axis] = error;
                worst_lag_sample[axis] = index;
            }
        }
    }
    assert_eq!(
        lag,
        [
            TRACK_SIMULATED_LAG_X_BASIS_POINTS,
            TRACK_SIMULATED_LAG_Y_BASIS_POINTS
        ],
        "the measured smoother lag changed"
    );
    assert!(
        lag[0] > worst_raw_jitter,
        "the smoothed curve must lag by more than the raw jitter it rejects, or this leg is not          exercising the lag at all: {} bp of lag against {worst_raw_jitter} bp of jitter",
        lag[0]
    );
    let lagged = assert_tracked_window_contains_the_subject(
        &smoothed_centres,
        &samples,
        KeyframeInterpolation::Linear,
        TRACK_HALF_WIDTH_BASIS_POINTS,
        TRACK_HALF_HEIGHT_BASIS_POINTS,
        "simulated_lagged_curve",
    );
    assert_eq!(lagged.frames_asserted, TRACK_FRAMES);
    assert_eq!(
        lagged.worst_frame, 99,
        "the worst lagged margin is at the last frame, which is §5.2's stated last-sample median          substitution"
    );
    assert!(
        (lagged.worst_margin_x_basis_points - TRACK_LAGGED_WORST_MARGIN_X_BASIS_POINTS).abs()
            <= 1.0e-6,
        "the measured lagged x margin is {} bp, not the recorded {} bp",
        lagged.worst_margin_x_basis_points,
        TRACK_LAGGED_WORST_MARGIN_X_BASIS_POINTS
    );
    assert!(
        (lagged.worst_margin_y_basis_points - TRACK_LAGGED_WORST_MARGIN_Y_BASIS_POINTS).abs()
            <= 1.0e-6,
        "the measured lagged y margin is {} bp, not the recorded {} bp",
        lagged.worst_margin_y_basis_points,
        TRACK_LAGGED_WORST_MARGIN_Y_BASIS_POINTS
    );
    // The contract's budget is `half_extent − worst_case_offset`: 675 bp in x
    // and 689 bp in y. Both measured curves must fit inside it — an assertion
    // about a measurement, not about arithmetic — and the lagged run shows how
    // much of it a legally noisy tracker actually spends: 192 bp of the 675 bp
    // x budget survives, so the budget is consumed by the raw tolerance and
    // the smoother lag together rather than by interpolation alone.
    for (label, margin, budget) in [
        (
            "analytic_x",
            analytic.worst_margin_x_basis_points,
            TRACK_HALF_WIDTH_BASIS_POINTS - 625,
        ),
        (
            "analytic_y",
            analytic.worst_margin_y_basis_points,
            TRACK_HALF_HEIGHT_BASIS_POINTS - 1_111,
        ),
        (
            "lagged_x",
            lagged.worst_margin_x_basis_points,
            TRACK_HALF_WIDTH_BASIS_POINTS - 625,
        ),
        (
            "lagged_y",
            lagged.worst_margin_y_basis_points,
            TRACK_HALF_HEIGHT_BASIS_POINTS - 1_111,
        ),
    ] {
        assert!(
            margin > 0.0 && margin <= budget as f64,
            "{label}: the measured margin {margin} bp must be positive and inside the contract's              {budget} bp budget"
        );
    }

    // --- the GPU agrees, at both layer scales -----------------------------
    let aspect = TRACK_WIDTH as f32 / TRACK_HEIGHT as f32;
    let gpu = fallback_gpu();
    let compositor = Compositor::new(gpu.context());
    let mut worst_code = 0_u8;
    for frame in &samples {
        let evaluated = document.tracks[0].clips[0].effects[0].evaluated_at(TimeCode(*frame));
        let resolved = Matte::from_params(&MatteParams::from_effect(&evaluated))
            .expect("the tracked matte is active");
        let coverage = compositor
            .render_matte(
                (TRACK_WIDTH, TRACK_HEIGHT),
                &[CompositorLayer {
                    frame: &frames[*frame as usize],
                    effects: std::slice::from_ref(&evaluated),
                    transition: TransitionRenderParams::default(),
                }],
                None,
                MatteRenderTarget {
                    layer_index: 0,
                    clip: ClipId(1),
                    effect: EffectId(1),
                },
            )
            .expect("the tracked coverage renders");
        for (index, actual) in coverage.iter().enumerate() {
            let x = index as u32 % TRACK_WIDTH;
            let y = index as u32 / TRACK_WIDTH;
            let uv = [
                (x as f32 + 0.5) / TRACK_WIDTH as f32,
                (y as f32 + 0.5) / TRACK_HEIGHT as f32,
            ];
            let expected = (resolved
                .coverage(uv, aspect, [0.5, 0.5, 0.5])
                .clamp(0.0, 1.0)
                * 255.0)
                .round() as u8;
            worst_code = worst_code.max(actual.abs_diff(expected));
        }
        assert!(
            worst_code <= MATTE_PROOF_FEATHERED_CODE_TOLERANCE,
            "frame {frame}: the GPU coverage diverges from the CPU reference by {worst_code} codes"
        );
    }

    // CC5 §5.2's coordinate space: the matte is evaluated at the *layer* uv,
    // so a scaled layer moves the coverage in the composite by
    // `u_composite = 0.5 + (u_layer − 0.5)·scale`. The tool must convert; the
    // fixture asserts the conversion the tool has to make.
    let probe = document.tracks[0].clips[0].effects[0].evaluated_at(TimeCode(50));
    let full = compositor
        .render_matte(
            (TRACK_WIDTH, TRACK_HEIGHT),
            &[CompositorLayer {
                frame: &frames[50],
                effects: std::slice::from_ref(&probe),
                transition: TransitionRenderParams::default(),
            }],
            None,
            MatteRenderTarget {
                layer_index: 0,
                clip: ClipId(1),
                effect: EffectId(1),
            },
        )
        .expect("the unscaled coverage renders");
    let scaled_effects = vec![
        color_node_effect(9, "transform", vec![("scale_percent".to_owned(), 50)]),
        probe.clone(),
    ];
    let scaled = compositor
        .render_matte(
            (TRACK_WIDTH, TRACK_HEIGHT),
            &[CompositorLayer {
                frame: &frames[50],
                effects: &scaled_effects,
                transition: TransitionRenderParams::default(),
            }],
            None,
            MatteRenderTarget {
                layer_index: 0,
                clip: ClipId(1),
                effect: EffectId(1),
            },
        )
        .expect("the half-scale coverage renders");
    let bounds = |coverage: &[u8]| {
        let mut bounds: Option<(f64, f64, f64, f64)> = None;
        for (index, code) in coverage.iter().enumerate() {
            if *code == 0 {
                continue;
            }
            let x = f64::from(index as u32 % TRACK_WIDTH);
            let y = f64::from(index as u32 / TRACK_WIDTH);
            bounds = Some(match bounds {
                None => (x, x, y, y),
                Some((left, right, top, bottom)) => {
                    (left.min(x), right.max(x), top.min(y), bottom.max(y))
                }
            });
        }
        bounds.expect("the window is on screen")
    };
    let (full_left, full_right, full_top, full_bottom) = bounds(&full);
    let (half_left, half_right, half_top, half_bottom) = bounds(&scaled);
    let map = |value: f64, extent: f64| extent * 0.5 + (value - extent * 0.5) * 0.5;
    for (label, actual, expected) in [
        ("left", half_left, map(full_left, f64::from(TRACK_WIDTH))),
        ("right", half_right, map(full_right, f64::from(TRACK_WIDTH))),
        ("top", half_top, map(full_top, f64::from(TRACK_HEIGHT))),
        (
            "bottom",
            half_bottom,
            map(full_bottom, f64::from(TRACK_HEIGHT)),
        ),
    ] {
        assert!(
            (actual - expected).abs() <= 1.0,
            "at scale 0.5 the coverage {label} edge is {actual}, and the §5.2 conversion predicts \
             {expected}"
        );
    }

    // --- §5.2's offset leg -------------------------------------------------
    //
    // The scale case above cannot see a sign error on the offset, because a
    // pure scale has no offset term. The forward map is
    //
    //     u_composite = scale·(u_layer − 0.5) + (offset_x, offset_y)/2 + 0.5
    //
    // and the compositor accumulates `params.offset_{x,y} += percent / 50`, so
    // `y_percent = +20` gives `offset_y = 0.4` and moves the picture DOWN by
    // `0.4/2 = 0.2` of the frame height: `0.2 · 360 = 72` px exactly. Likewise
    // `x_percent = +20` moves it RIGHT by `0.2 · 640 = 128` px exactly. The
    // vertex shader's `−offset_y` in NDC is already absorbed by the
    // `uv.y = (1 − ndc.y)/2` flip, so there is **no** extra sign on
    // `offset_y` — which is exactly what this leg exists to pin.
    //
    // Frame 0 is used rather than frame 50 because the analytic centre is
    // `(5000, 5000)` there, so the shifted boxes stay well inside the raster
    // and the expectation is an integer pixel translation rather than a
    // clipped one.
    let at_zero = document.tracks[0].clips[0].effects[0].evaluated_at(TimeCode::ZERO);
    let transformed_bounds = |parameters: &[(&str, i64)]| {
        let mut effects = Vec::new();
        if !parameters.is_empty() {
            effects.push(color_node_effect(
                9,
                "transform",
                parameters
                    .iter()
                    .map(|(name, value)| ((*name).to_owned(), *value))
                    .collect(),
            ));
        }
        effects.push(at_zero.clone());
        let coverage = compositor
            .render_matte(
                (TRACK_WIDTH, TRACK_HEIGHT),
                &[CompositorLayer {
                    frame: &frames[0],
                    effects: &effects,
                    transition: TransitionRenderParams::default(),
                }],
                None,
                MatteRenderTarget {
                    layer_index: 0,
                    clip: ClipId(1),
                    effect: EffectId(1),
                },
            )
            .expect("the transformed coverage renders");
        bounds(&coverage)
    };
    let identity = transformed_bounds(&[]);
    // The window at frame 0 is centred with `hw = 1300`, `hh = 1800`, so it
    // spans `u.x ∈ [0.37, 0.63]` and `u.y ∈ [0.32, 0.68]`. In pixels that is
    // `x ∈ [236.8, 403.2]` and `y ∈ [115.2, 244.8]`, and a pixel centre
    // `(p + 0.5)` is inside exactly when `p ∈ 237..=402` and `p ∈ 115..=244`
    // — hand-derived from §2.3.
    assert_eq!(
        identity,
        (237.0, 402.0, 115.0, 244.0),
        "the untransformed coverage box at frame 0 is not the hand-derived rectangle"
    );
    /// `0.2 · 640`: the composite shift `x_percent = 20` must produce.
    const OFFSET_X_SHIFT_PIXELS: f64 = 128.0;
    /// `0.2 · 360`: the composite shift `y_percent = 20` must produce, DOWN.
    const OFFSET_Y_SHIFT_PIXELS: f64 = 72.0;
    assert_eq!(OFFSET_X_SHIFT_PIXELS, 0.2 * f64::from(TRACK_WIDTH));
    assert_eq!(OFFSET_Y_SHIFT_PIXELS, 0.2 * f64::from(TRACK_HEIGHT));
    let offset_down = transformed_bounds(&[("scale_percent", 100), ("y_percent", 20)]);
    assert_eq!(
        offset_down,
        (
            identity.0,
            identity.1,
            identity.2 + OFFSET_Y_SHIFT_PIXELS,
            identity.3 + OFFSET_Y_SHIFT_PIXELS
        ),
        "y_percent = +20 at scale 1 must move the coverage DOWN by exactly {OFFSET_Y_SHIFT_PIXELS} \
         px and not move it in x; an extra sign on offset_y would move it UP"
    );
    let offset_right = transformed_bounds(&[("scale_percent", 100), ("x_percent", 20)]);
    assert_eq!(
        offset_right,
        (
            identity.0 + OFFSET_X_SHIFT_PIXELS,
            identity.1 + OFFSET_X_SHIFT_PIXELS,
            identity.2,
            identity.3
        ),
        "x_percent = +20 at scale 1 must move the coverage RIGHT by exactly \
         {OFFSET_X_SHIFT_PIXELS} px and not move it in y"
    );
    let offset_up = transformed_bounds(&[("scale_percent", 100), ("y_percent", -20)]);
    assert_eq!(
        offset_up,
        (
            identity.0,
            identity.1,
            identity.2 - OFFSET_Y_SHIFT_PIXELS,
            identity.3 - OFFSET_Y_SHIFT_PIXELS
        ),
        "y_percent = -20 is the exact mirror of +20"
    );
    // The combined case, asserted against the forward map itself rather than
    // against a shift, so scale and offset cannot cancel each other's sign.
    let forward = |layer_pixel: f64, extent: f64, scale: f64, offset: f64| {
        let u_layer = (layer_pixel + 0.5) / extent;
        let u_composite = scale * (u_layer - 0.5) + offset / 2.0 + 0.5;
        u_composite * extent - 0.5
    };
    let combined =
        transformed_bounds(&[("scale_percent", 50), ("x_percent", 20), ("y_percent", 20)]);
    for (label, actual, expected) in [
        (
            "left",
            combined.0,
            forward(identity.0, f64::from(TRACK_WIDTH), 0.5, 0.4),
        ),
        (
            "right",
            combined.1,
            forward(identity.1, f64::from(TRACK_WIDTH), 0.5, 0.4),
        ),
        (
            "top",
            combined.2,
            forward(identity.2, f64::from(TRACK_HEIGHT), 0.5, 0.4),
        ),
        (
            "bottom",
            combined.3,
            forward(identity.3, f64::from(TRACK_HEIGHT), 0.5, 0.4),
        ),
    ] {
        assert!(
            (actual - expected).abs() <= 1.0,
            "at scale 0.5 with +20 % offsets the coverage {label} edge is {actual}, and the §5.2 \
             forward map predicts {expected}"
        );
    }
    // …and against the hand-derived pixel box, so a fixture that reproduced
    // the shader's sign error in its own formula could not pass either.
    // `u.x ∈ [0.37, 0.63]` maps to `[0.635, 0.765]` → `x ∈ 406..=489`;
    // `u.y ∈ [0.32, 0.68]` maps to `[0.61, 0.79]` → `y ∈ 220..=283`.
    assert_eq!(
        combined,
        (406.0, 489.0, 220.0, 283.0),
        "the combined scale-and-offset coverage box is not the hand-derived rectangle"
    );

    emit_cc5_evidence(
        "cc5_tracked_shot",
        gpu.backend(),
        gpu.lane.id(),
        json!({
            "source": "640x360 25 fps, 100 frames: a solid 0x303030 background with an 80x80 white overlay on x = 320 + 120 sin(2πt/8) − 40, y = 180 + 60 sin(2πt/8) − 40",
            "sample_frames": samples,
            "step_frames": TRACK_STEP_FRAMES,
            "window": {
                "half_width_basis_points": TRACK_HALF_WIDTH_BASIS_POINTS,
                "half_height_basis_points": TRACK_HALF_HEIGHT_BASIS_POINTS,
            },
            "layer_scales": [0.5, 1.0],
            "tool_half": "track_matte_window belongs to kinewright-agent; see the manifest owners",
        }),
        (TRACK_WIDTH, TRACK_HEIGHT),
        output_hash(&full),
        json!({
            "frames_asserted": TRACK_FRAMES,
            "measured_boxes": measured_boxes,
            "worst_containment_margin_x_basis_points": analytic.worst_margin_x_basis_points,
            "worst_containment_margin_y_basis_points": analytic.worst_margin_y_basis_points,
            "worst_containment_margin_frame": analytic.worst_frame,
            "derived_margin_x_basis_points": TRACK_HALF_WIDTH_BASIS_POINTS - 625,
            "derived_margin_y_basis_points": TRACK_HALF_HEIGHT_BASIS_POINTS - 1_111,
            "subject_pixels_asserted": analytic.pixels_asserted,
            "simulated_lag": {
                "raw_jitter_basis_points": worst_raw_jitter,
                "raw_tolerance_basis_points": 200,
                "dead_zone_basis_points": MATTE_TRACK_DEAD_ZONE_BASIS_POINTS,
                "maximum_step_basis_points": MATTE_TRACK_MAX_STEP_BASIS_POINTS,
                "smoother": "kinewright_core::stabilize_tracked_centres_basis_points (M40 three-sample median filter plus the reactive controller)",
                "measured_lag_x_basis_points": lag[0],
                "measured_lag_y_basis_points": lag[1],
                "worst_lag_sample_index": worst_lag_sample,
                "worst_containment_margin_x_basis_points": lagged.worst_margin_x_basis_points,
                "worst_containment_margin_y_basis_points": lagged.worst_margin_y_basis_points,
                "worst_containment_margin_frame": lagged.worst_frame,
                "note": "the lag peaks at the last sample, which is CC5 §5.2's stated median substitution; 192 of the 675 bp x budget survives, so the budget is spent on the raw tolerance and the lag together",
            },
            "gpu_worst_code_difference": worst_code,
            "coverage_bounding_box_scale_1": [full_left, full_right, full_top, full_bottom],
            "coverage_bounding_box_scale_0_5": [half_left, half_right, half_top, half_bottom],
        }),
    );
}

// ---------------------------------------------------------------------------
// §9.0.4: the manifest.
// ---------------------------------------------------------------------------

/// Every CC5 test this file owns. The manifest is asserted to claim all of
/// them, so a fixture cannot be orphaned or a manifest entry invented.
const CC5_MEDIA_TESTS: [&str; 22] = [
    "cc5_rasters_cover_their_controls_and_land_on_pixel_centres",
    "cc5_every_matte_control_bound_matches_a_hand_derived_expected_value",
    "cc5_degenerate_window_half_extents_weigh_exactly_zero",
    "cc5_affected_pixel_containment_is_exact_on_cpu_and_gpu",
    "cc5_window_geometry_anchors_are_hand_derived_on_cpu_and_gpu",
    "cc5_feather_anchors_and_symmetry_match_the_contract",
    "cc5_window_combine_is_hand_derived_on_cpu_and_gpu",
    "cc5_qualifier_anchors_match_the_hand_derived_values",
    "cc5_mix_and_invert_scale_the_coverage_exactly",
    "cc5_keyframed_window_motion_moves_the_covered_set",
    "cc5_gpu_compositor_matches_the_cpu_reference_on_software_fallback",
    "cc5_gpu_compositor_matches_the_cpu_reference_on_hardware",
    "cc5_matte_proof_matches_the_cpu_reference_coverage",
    "cc5_matte_scoped_scopes_measure_exactly_the_affected_set",
    "cc5_tracked_shot_window_contains_the_subject_at_every_frame",
    "cc5_migration_is_bit_identical_and_the_mask_never_interacts",
    "cc5_buffer_layout_limits_and_abi_constants_hold",
    "cc5_performance_evidence_is_recorded_on_software_fallback",
    "cc5_performance_evidence_is_recorded_on_hardware",
    "cc5_skin_and_product_qualifiers_select_only_their_patch",
    "cc5_manifest_declares_every_required_fixture_and_constant",
    "cc5_declared_test_names_exist_in_their_source_files",
];

/// The §9.2 items whose evidence lives outside this crate.
const CC5_EXTERNAL_OWNERS: [(u64, &str); 4] = [
    (10, "kinewright-agent"),
    (11, "kinewright-agent"),
    (14, "kinewright-core"),
    (15, "kinewright-agent"),
];

/// Every CC5 test [`engine.rs`](crate::engine)'s `#[cfg(test)]` module owns.
///
/// They live there rather than here because they drive `FfmpegMediaEngine`'s
/// `matte_proof_for_document` through the production engine seam, which is
/// private to that module. They carry the `cc5_` prefix so
/// `cargo test -p kinewright-media --lib -- cc5` really runs the whole slice.
const CC5_ENGINE_TESTS: [&str; 5] = [
    "cc5_matte_proof_reports_exact_window_coverage_and_metadata",
    "cc5_matte_proof_fails_typed_instead_of_returning_a_blank_frame",
    "cc5_matte_proof_refuses_a_clip_that_is_not_visible_at_the_frame",
    "cc5_matte_proof_ignores_a_layer_above_the_target_clip",
    "cc5_matte_proof_follows_a_keyframed_window_center",
];

/// The CC5 tests that live in `compositor.rs` next to the ABI they pin
/// (§9.2 items 8, 12, and 13). They are not `cc5_`-prefixed because two of
/// them are word-map and migration unit tests of the serializer rather than
/// lane fixtures, so the inventory names them explicitly.
const CC5_COMPOSITOR_TESTS: [&str; 3] = [
    "cc5_matte_gpu_anchors_hold_on_hardware",
    "matte_block_layout_is_the_cc5_word_map",
    "a_pre_cc5_document_renders_bit_identically",
];

/// The two §9.0.4 inventory tests, which are fixture-quality rules rather than
/// §9.2 items and are therefore claimed by `manifest_self_test` rather than by
/// a numbered fixture.
const CC5_INVENTORY_TESTS: [&str; 2] = [
    "cc5_manifest_declares_every_required_fixture_and_constant",
    "cc5_declared_test_names_exist_in_their_source_files",
];

/// The sources every declared CC5 test name is verified against, keyed by the
/// workspace-relative path the manifest names.
///
/// `include_str!` rather than a runtime read on purpose: the check becomes a
/// **compile-time** dependency, so renaming a test in the core or agent crate
/// rebuilds this fixture and fails it, instead of leaving a manifest entry
/// that names a function nobody has written for three commits. The prose
/// entries this replaced ("`track_matte_window` tracking tests") could not fail
/// at all.
const CC5_TEST_SOURCES: [(&str, &str); 9] = [
    (
        "crates/kinewright-media/src/cc5_fixtures.rs",
        include_str!("cc5_fixtures.rs"),
    ),
    (
        "crates/kinewright-media/src/engine.rs",
        include_str!("engine.rs"),
    ),
    (
        "crates/kinewright-media/src/compositor.rs",
        include_str!("compositor.rs"),
    ),
    (
        "crates/kinewright-core/tests/cc5_core.rs",
        include_str!("../../kinewright-core/tests/cc5_core.rs"),
    ),
    (
        "crates/kinewright-core/tests/cc5_core_proof.rs",
        include_str!("../../kinewright-core/tests/cc5_core_proof.rs"),
    ),
    (
        "crates/kinewright-agent/src/server.rs",
        include_str!("../../kinewright-agent/src/server.rs"),
    ),
    (
        "crates/kinewright-agent/src/color_status.rs",
        include_str!("../../kinewright-agent/src/color_status.rs"),
    ),
    (
        "crates/kinewright-agent/src/color_scopes.rs",
        include_str!("../../kinewright-agent/src/color_scopes.rs"),
    ),
    (
        "crates/kinewright-agent/tests/mcp_server.rs",
        include_str!("../../kinewright-agent/tests/mcp_server.rs"),
    ),
];

/// One source's text, or a panic naming the path the manifest invented.
fn cc5_test_source(path: &str) -> &'static str {
    CC5_TEST_SOURCES
        .iter()
        .find_map(|(candidate, source)| (*candidate == path).then_some(*source))
        .unwrap_or_else(|| {
            panic!(
                "the manifest names source {path}, which cc5_fixtures.rs does not include; add it \
                 to CC5_TEST_SOURCES"
            )
        })
}

/// Whether `source` declares `name` as a `#[test]` (or `#[tokio::test]`)
/// function.
///
/// The attribute is required, so a name mentioned in a doc comment, a string
/// literal, or a helper function is not mistaken for a fixture. Attribute,
/// comment, and blank lines between the attribute and the signature are
/// skipped, because `#[ignore = "…"]` and a doc comment routinely sit there.
fn is_test_attribute(line: &str) -> bool {
    line == "#[test]" || line.starts_with("#[tokio::test")
}

fn declares_test(source: &str, name: &str) -> bool {
    let needle = format!("fn {name}(");
    let lines = source.lines().collect::<Vec<_>>();
    for (index, line) in lines.iter().enumerate() {
        if !line.contains(&needle) {
            continue;
        }
        for previous in lines[..index].iter().rev() {
            let previous = previous.trim();
            if is_test_attribute(previous) {
                return true;
            }
            if previous.is_empty() || previous.starts_with("//") || previous.starts_with("#[") {
                continue;
            }
            break;
        }
    }
    false
}

/// Every `#[test]` function in `source` whose name starts with `prefix`, in
/// declaration order.
fn declared_test_names(source: &str, prefix: &str) -> Vec<String> {
    let lines = source.lines().collect::<Vec<_>>();
    let mut names = Vec::new();
    for (index, line) in lines.iter().enumerate() {
        if !is_test_attribute(line.trim()) {
            continue;
        }
        for candidate in &lines[index + 1..] {
            let candidate = candidate.trim();
            if candidate.is_empty() || candidate.starts_with("//") || candidate.starts_with("#[") {
                continue;
            }
            let Some(rest) = candidate.split_once("fn ").map(|(_, rest)| rest) else {
                break;
            };
            let Some((name, _)) = rest.split_once('(') else {
                break;
            };
            if name.starts_with(prefix) {
                names.push(name.to_owned());
            }
            break;
        }
    }
    names
}

/// CC5 §9.0.4. The declared test inventories are tied to the source they claim
/// to describe, in **both** directions: every name this file lists exists as a
/// `#[test]` function in the file that owns it, and every `cc5_*` test the
/// media sources declare is listed.
///
/// The lists used to be hand-written prose compared against a hand-written
/// manifest, so two matching typos passed and an unclaimed fixture was
/// invisible.
#[test]
fn cc5_declared_test_names_exist_in_their_source_files() {
    let fixtures = cc5_test_source("crates/kinewright-media/src/cc5_fixtures.rs");
    let engine = cc5_test_source("crates/kinewright-media/src/engine.rs");

    // --- both directions, for the two media sources -----------------------
    let declared_here = declared_test_names(fixtures, "cc5_");
    let mut expected_here = CC5_MEDIA_TESTS.map(str::to_owned).to_vec();
    expected_here.sort_unstable();
    let mut actual_here = declared_here.clone();
    actual_here.sort_unstable();
    assert_eq!(
        actual_here, expected_here,
        "CC5_MEDIA_TESTS and the `cc5_*` tests cc5_fixtures.rs actually declares disagree"
    );
    let declared_in_engine = declared_test_names(engine, "cc5_");
    let mut expected_in_engine = CC5_ENGINE_TESTS.map(str::to_owned).to_vec();
    expected_in_engine.sort_unstable();
    let mut actual_in_engine = declared_in_engine.clone();
    actual_in_engine.sort_unstable();
    assert_eq!(
        actual_in_engine, expected_in_engine,
        "CC5_ENGINE_TESTS and the `cc5_*` tests engine.rs actually declares disagree"
    );
    // The engine tests must match the `cc5` filter and must not be silently
    // skippable: they take the panicking `fallback_gpu()` convention, not
    // `fixture_gpu_or_skip()`, which passes when the skip opt-in is set.
    assert!(
        !engine.contains("fixture_gpu_or_skip"),
        "engine.rs must not use the silently-skipping GPU helper; the CC5 proofs use \
         fallback_gpu(), which panics when no adapter exists"
    );
    for name in CC5_ENGINE_TESTS {
        assert!(
            name.starts_with("cc5_"),
            "{name} does not match the `cargo test -- cc5` filter"
        );
    }

    // --- every name the manifest claims exists in the source it names -----
    let manifest: Value = serde_json::from_str(include_str!("../tests/fixtures/cc5_manifest.json"))
        .expect("CC5 fixture manifest must be valid JSON");
    let mut verified = 0_usize;
    let items = manifest["required_fixtures"]
        .as_array()
        .expect("the manifest must list the §9.2 items");
    for item in items {
        let number = item["item"].as_u64().expect("a numbered item");
        for owner in item["owners"].as_array().expect("owners") {
            let crate_name = owner["owner"].as_str().expect("an owner crate name");
            let sources = owner["sources"]
                .as_array()
                .unwrap_or_else(|| {
                    panic!("§9.2 item {number} owner {crate_name} must name its source files")
                })
                .iter()
                .map(|path| {
                    let path = path.as_str().expect("a source path");
                    assert!(
                        path.starts_with(&format!("crates/{crate_name}/")),
                        "§9.2 item {number} owner {crate_name} names source {path}, which is not \
                         in that crate"
                    );
                    cc5_test_source(path)
                })
                .collect::<Vec<_>>();
            for test in owner["tests"].as_array().expect("tests") {
                let name = test.as_str().expect("a test name");
                assert!(
                    sources.iter().any(|source| declares_test(source, name)),
                    "§9.2 item {number} owner {crate_name} claims a test named {name}, which none \
                     of its declared sources declares as a #[test] function"
                );
                verified += 1;
            }
        }
    }
    // The two §9.0.4 inventory tests are claimed by `manifest_self_test`
    // rather than by a numbered item, so they are checked separately.
    for name in CC5_INVENTORY_TESTS {
        assert!(
            declares_test(fixtures, name),
            "the §9.0.4 inventory test {name} is not declared in cc5_fixtures.rs"
        );
        verified += 1;
    }
    assert_eq!(
        manifest["manifest_self_test"]["test"], CC5_INVENTORY_TESTS[0],
        "the manifest must name the test that asserts it against the code"
    );
    assert_eq!(
        manifest["manifest_self_test"]["inventory_test"], CC5_INVENTORY_TESTS[1],
        "the manifest must name the test that ties its declared test names to their sources"
    );
    // A count, so a manifest that quietly emptied its `tests` arrays cannot
    // pass this test vacuously.
    assert!(
        verified >= 50,
        "only {verified} declared test names were verified; the manifest has lost entries"
    );
}

/// CC5 §9.0.4 and §9.2. Every required fixture is declared with its owner, and
/// every declared tolerance and constant is asserted equal to the code the
/// fixtures actually gate with.
#[test]
fn cc5_manifest_declares_every_required_fixture_and_constant() {
    let manifest: Value = serde_json::from_str(include_str!("../tests/fixtures/cc5_manifest.json"))
        .expect("CC5 fixture manifest must be valid JSON");
    assert_eq!(manifest["manifest_version"], 1);
    assert_eq!(manifest["contract"], "CC5 secondaries");
    assert_eq!(manifest["contract_token"], CC5_CONTRACT);
    assert_eq!(
        manifest["nodes"],
        json!(kinewright_core::MANAGED_COLOR_NODE_NAMES)
    );

    // --- §2.1 which kinds may carry a matte -------------------------------
    for name in kinewright_core::MANAGED_COLOR_NODE_NAMES {
        let declared = manifest["matte_capable_nodes"][name]
            .as_bool()
            .unwrap_or_else(|| panic!("the manifest must declare whether {name} carries a matte"));
        assert_eq!(
            declared,
            kinewright_core::is_matte_capable_color_node(name),
            "{name}: the manifest and Core disagree about matte capability"
        );
        let descriptor = effect_descriptor(name).expect("a managed descriptor");
        assert_eq!(
            manifest["descriptor_sizes"][name],
            descriptor.parameters.len(),
            "{name} descriptor size"
        );
        let matte_parameters = descriptor
            .parameters
            .iter()
            .filter(|parameter| is_matte_parameter(parameter.name))
            .count();
        assert_eq!(
            matte_parameters,
            if declared { MATTE_PARAMETER_COUNT } else { 0 },
            "{name} carries the wrong number of matte parameters"
        );
    }

    // --- §2.2 the generated control tables --------------------------------
    assert_eq!(manifest["matte_parameter_count"], MATTE_PARAMETER_COUNT);
    assert_eq!(manifest["matte_window_limit"], MATTE_WINDOW_LIMIT);
    let descriptor = effect_descriptor("color_wheels").expect("the descriptor exists");
    let names = matte_parameter_names();
    let controls = manifest["matte_controls"]
        .as_array()
        .expect("the manifest must declare the 15 matte controls");
    assert_eq!(controls.len(), 15);
    let windows = manifest["matte_window_controls"]
        .as_array()
        .expect("the manifest must declare the 8 window controls");
    assert_eq!(windows.len(), 8);
    for (declared, name) in controls.iter().chain(windows).zip(names.iter()) {
        let parameter = descriptor
            .parameter(name)
            .unwrap_or_else(|| panic!("color_wheels must carry {name}"));
        assert_eq!(declared["name"], *name);
        assert_eq!(declared["min"], parameter.min, "{name} min");
        assert_eq!(declared["max"], parameter.max, "{name} max");
        assert_eq!(declared["neutral"], parameter.neutral, "{name} neutral");
    }
    // Window 0's names are the generated table's, and every window repeats it.
    for window in 0..MATTE_WINDOW_LIMIT {
        let generated = matte_window_parameter_names(window).expect("a window inside the limit");
        for (declared, name) in windows.iter().zip(generated) {
            let declared = declared["name"].as_str().expect("a control name");
            assert_eq!(
                declared.replace("window0", &format!("window{window}")),
                *name,
                "window {window} does not repeat the generated table"
            );
        }
    }

    // --- §9.1 rasters ------------------------------------------------------
    let rasters = &manifest["rasters"];
    assert_eq!(rasters["width"], CC5_RASTER_WIDTH);
    assert_eq!(rasters["height"], CC5_RASTER_HEIGHT);
    assert_eq!(rasters["parity_block_width"], CC5_PARITY_BLOCK_WIDTH);
    assert_eq!(rasters["parity_block_height"], CC5_PARITY_BLOCK_HEIGHT);
    assert_eq!(rasters["centred_window_pixels"], CENTRED_WINDOW_PIXELS);
    assert_eq!(
        rasters["centred_window_outside_pixels"],
        CENTRED_WINDOW_OUTSIDE_PIXELS
    );
    assert_eq!(
        rasters["centred_window_basis_points"],
        CENTRED_WINDOW_BASIS_POINTS
    );

    // --- §9.2.2 and §9.2.4 anchors, recomputed from the transcription -----
    let raster = cc5_field_raster();
    let anchors = &manifest["window_anchors"];
    let count = |matte: &MatteSpec| {
        matte
            .covered_pixels(&raster)
            .iter()
            .filter(|covered| **covered)
            .count()
    };
    let a = WindowSpec::CENTRED;
    let b = WindowSpec::CENTRED.with_centre(7_500, 5_000);
    for (key, matte) in [
        ("centred_rect_2500", MatteSpec::window(a)),
        (
            "pixel_square_rect_rotation_0",
            MatteSpec::window(WindowSpec::PIXEL_SQUARE),
        ),
        (
            "pixel_square_rect_rotation_4500",
            MatteSpec::window(WindowSpec::PIXEL_SQUARE.with_rotation(4_500)),
        ),
        (
            "pixel_square_ellipse",
            MatteSpec::window(WindowSpec::PIXEL_SQUARE.with_shape(SHAPE_ELLIPSE)),
        ),
        (
            "union",
            MatteSpec::window(a).with_windows(vec![a, b], COMBINE_UNION),
        ),
        (
            "intersection",
            MatteSpec::window(a).with_windows(vec![a, b], COMBINE_INTERSECTION),
        ),
        (
            "union_with_inverted_window",
            MatteSpec::window(a).with_windows(vec![a, b.inverted()], COMBINE_UNION),
        ),
    ] {
        assert_eq!(
            anchors[key],
            count(&matte),
            "the manifest anchor {key} disagrees with the §2.3 transcription"
        );
    }

    // --- §3.1 buffer and ABI ----------------------------------------------
    let buffer = &manifest["buffer"];
    assert_eq!(
        buffer["compositor_required_storage_buffer_binding_size"],
        COMPOSITOR_REQUIRED_STORAGE_BUFFER_BINDING_SIZE
    );
    assert_eq!(
        buffer["compositor_required_storage_buffers_per_shader_stage"],
        u64::from(COMPOSITOR_REQUIRED_STORAGE_BUFFERS_PER_SHADER_STAGE)
    );
    assert_eq!(
        buffer["grade_buffer_worst_case_bytes"],
        GRADE_BUFFER_WORST_CASE_BYTES
    );
    assert_eq!(buffer["matte_block_words"], MATTE_BLOCK_WORDS);
    assert_eq!(buffer["curve_payload_words"], GRADE_CURVE_PAYLOAD_WORDS);
    assert_eq!(
        buffer["matte_payload_offset_word"],
        GRADE_NODE_MATTE_OFFSET_WORD
    );
    // `GRADE_ABI_VERSION` is private to the compositor, so the manifest is
    // asserted against the version the production serializer actually writes
    // into `header.z` rather than against a restated literal.
    let empty = crate::compositor::grade_buffer_bytes_for(&[], None, CC5_RESOLUTION, None)
        .expect("an empty stack serializes");
    assert_eq!(
        buffer["grade_abi_version"],
        u64::from(grade_header_word(&empty, 2))
    );
    assert_eq!(buffer["grade_abi_version"], 3);
    assert!(
        buffer["grade_buffer_worst_case_bytes"]
            .as_u64()
            .expect("a numeric worst case")
            <= COMPOSITOR_REQUIRED_STORAGE_BUFFER_BINDING_SIZE
    );

    // --- §9.0.4 tolerances are the code constants -------------------------
    let tolerances = &manifest["tolerances"];
    assert_manifest_f64(
        tolerances,
        "monitor_cpu_gpu_max_code",
        f64::from(MONITOR_CPU_GPU_MAX),
    );
    assert_manifest_f64(tolerances, "monitor_cpu_gpu_p99_code", MONITOR_CPU_GPU_P99);
    assert_manifest_f64(
        tolerances,
        "monitor_cpu_gpu_mean_code",
        MONITOR_CPU_GPU_MEAN,
    );
    assert_manifest_f32(tolerances, "linear_cpu_gpu_max", LINEAR_CPU_GPU_MAX);
    assert_manifest_f32(tolerances, "linear_cpu_gpu_p99", LINEAR_CPU_GPU_P99);
    assert_manifest_f32(tolerances, "linear_cpu_gpu_mean", LINEAR_CPU_GPU_MEAN);
    assert_manifest_f32(tolerances, "linear_over_range_p99", LINEAR_OVER_RANGE_P99);
    assert_manifest_f32(tolerances, "linear_over_range_mean", LINEAR_OVER_RANGE_MEAN);
    assert_manifest_f32(tolerances, "linear_gate_in_gamut", LINEAR_GATE_IN_GAMUT);
    assert_manifest_f32(tolerances, "linear_gate_domain", LINEAR_GATE_DOMAIN);
    assert_manifest_f64(
        tolerances,
        "minimum_changed_linear_basis_points",
        MIN_CHANGED_LINEAR_BASIS_POINTS as f64,
    );
    assert_manifest_f32(
        tolerances,
        "feather_non_dyadic",
        FEATHER_NON_DYADIC_TOLERANCE,
    );
    assert_manifest_f32(tolerances, "qualifier_anchor", QUALIFIER_ANCHOR_TOLERANCE);
    assert_eq!(
        tolerances["matte_proof_feathered_code"],
        MATTE_PROOF_FEATHERED_CODE_TOLERANCE
    );
    assert!(
        tolerances["outside_the_matte"]
            .as_str()
            .is_some_and(|rule| rule.contains("no tolerance")),
        "the manifest must state that no tolerance excuses a changed pixel outside a matte"
    );

    // --- §9.2.11 tracking, whose constants the agent crate owns -----------
    let tracking = &manifest["tracking"];
    assert_eq!(tracking["owner"], "kinewright-agent");
    assert_eq!(tracking["sample_frames"], json!(tracking_sample_frames()));
    assert_eq!(tracking["step_frames"], TRACK_STEP_FRAMES);
    assert_eq!(
        tracking["half_width_basis_points"],
        TRACK_HALF_WIDTH_BASIS_POINTS
    );
    assert_eq!(
        tracking["half_height_basis_points"],
        TRACK_HALF_HEIGHT_BASIS_POINTS
    );
    assert_eq!(
        tracking["derived_margin_x_basis_points"],
        TRACK_HALF_WIDTH_BASIS_POINTS - 625
    );
    assert_eq!(
        tracking["derived_margin_y_basis_points"],
        TRACK_HALF_HEIGHT_BASIS_POINTS - 1_111
    );
    assert!(
        tracking["constants_asserted_by"]
            .as_str()
            .is_some_and(|note| note.contains("kinewright-agent")),
        "the tracking tolerances live in the agent crate, which media does not link; the manifest \
         must say so rather than restate them as media constants"
    );

    // --- §9.2.16 performance ----------------------------------------------
    let performance = &manifest["performance"];
    assert_eq!(
        performance["resolution"],
        json!([PERFORMANCE_RESOLUTION.0, PERFORMANCE_RESOLUTION.1])
    );
    assert_manifest_f64(
        performance,
        "soft_budget_milliseconds",
        PERFORMANCE_SOFT_BUDGET_MILLISECONDS,
    );
    assert!(
        performance["gate"]
            .as_str()
            .is_some_and(|gate| gate.contains("recorded evidence only")),
        "§9.2.16 is recorded evidence, never a hard gate"
    );
    // §9.2.16 is *recorded evidence*, so the field names the evidence payload
    // uses are part of the contract: a renamed field silently breaks whatever
    // reads the artefact to see a regression. The manifest declares them and
    // `record_cc5_performance` asserts the payload it emits carries exactly
    // these keys.
    assert_eq!(
        performance["evidence_fields"],
        json!(PERFORMANCE_EVIDENCE_FIELDS),
        "the manifest and the emitted §9.2.16 evidence disagree about the measurement field names"
    );

    // --- the fixture inventory --------------------------------------------
    assert_eq!(
        manifest["required_evidence"],
        json!(CC5_EVIDENCE_FIXTURES),
        "the manifest evidence list must match the emitted fixture names exactly"
    );
    let items = manifest["required_fixtures"]
        .as_array()
        .expect("the manifest must map the §9.2 items to owners and test names");
    assert_eq!(items.len(), 17, "§9.2 lists seventeen required fixtures");
    let mut declared_media_tests = Vec::new();
    for (index, item) in items.iter().enumerate() {
        let number = index as u64 + 1;
        assert_eq!(item["item"], number);
        assert!(
            item["name"].as_str().is_some_and(|name| !name.is_empty()),
            "§9.2 item {number} must be named"
        );
        let owners = item["owners"]
            .as_array()
            .unwrap_or_else(|| panic!("§9.2 item {number} must declare its owners"));
        assert!(!owners.is_empty(), "§9.2 item {number} must have an owner");
        for owner in owners {
            let crate_name = owner["owner"].as_str().expect("an owner crate name");
            assert!(
                [
                    "kinewright-media",
                    "kinewright-core",
                    "kinewright-app",
                    "kinewright-agent"
                ]
                .contains(&crate_name),
                "§9.2 item {number} names an unknown owner {crate_name}"
            );
            let tests = owner["tests"]
                .as_array()
                .unwrap_or_else(|| panic!("§9.2 item {number} must name its tests"));
            assert!(
                !tests.is_empty(),
                "§9.2 item {number} owner {crate_name} must name at least one test"
            );
            assert!(
                owner["scope"]
                    .as_str()
                    .is_some_and(|scope| !scope.is_empty()),
                "§9.2 item {number} owner {crate_name} must state what it covers"
            );
            if crate_name == "kinewright-media" {
                for test in tests {
                    let name = test.as_str().expect("a test name");
                    assert!(
                        CC5_MEDIA_TESTS.contains(&name)
                            || CC5_ENGINE_TESTS.contains(&name)
                            || CC5_COMPOSITOR_TESTS.contains(&name),
                        "§9.2 item {number} names media test {name}, which none of \
                         cc5_fixtures.rs, engine.rs, or compositor.rs contains"
                    );
                    declared_media_tests.push(name.to_owned());
                }
            }
        }
    }
    // The manifest must account for every media test, not merely a subset.
    // The two inventory tests are §9.0.4 rather than §9.2 items, so they are
    // claimed by `manifest_self_test` and declared here.
    for name in CC5_INVENTORY_TESTS {
        declared_media_tests.push(name.to_owned());
    }
    for name in CC5_MEDIA_TESTS
        .iter()
        .chain(CC5_ENGINE_TESTS.iter())
        .chain(CC5_COMPOSITOR_TESTS.iter())
    {
        assert!(
            declared_media_tests.iter().any(|declared| declared == name),
            "media test {name} is not claimed by any §9.2 item in the manifest"
        );
    }
    assert_eq!(
        manifest["manifest_self_test"]["test"], CC5_INVENTORY_TESTS[0],
        "the manifest must name the test that asserts it against the code"
    );
    assert_eq!(
        manifest["manifest_self_test"]["inventory_test"], CC5_INVENTORY_TESTS[1],
        "the manifest must name the test that ties its declared test names to their sources"
    );
    assert!(
        manifest["manifest_self_test"]["rule"]
            .as_str()
            .is_some_and(|rule| rule.contains("9.0.4")),
        "the manifest must cite the fixture-quality rule it satisfies"
    );

    // The items this crate does not own must say so by name.
    for (number, owner) in CC5_EXTERNAL_OWNERS {
        let item = &items[number as usize - 1];
        assert!(
            item["owners"]
                .as_array()
                .expect("owners")
                .iter()
                .any(|entry| entry["owner"] == owner),
            "§9.2 item {number} must record {owner} as an owner"
        );
    }
    // Item 15 is the agent's alone; media claims none of it.
    assert!(
        items[14]["owners"]
            .as_array()
            .expect("owners")
            .iter()
            .all(|entry| entry["owner"] != "kinewright-media"),
        "§9.2 item 15 is the agent's plan-not-apply evidence and is not owned by media"
    );

    for lane in ["software", "software_unavailable_opt_in", "hardware"] {
        assert!(
            manifest["gpu_contexts"][lane].is_string(),
            "the manifest must describe the {lane} GPU lane"
        );
    }
}
