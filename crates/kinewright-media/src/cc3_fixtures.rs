//! Objective CC3 evidence fixtures for `docs/CC3-CURVES-AND-WHEELS.md` §10.
//!
//! These fixtures live inside the media crate for the same reason the CC1
//! fixtures do: the `Rgba16Float` working frame and the production compositor
//! are internal seams, and the evidence has to exercise the real GPU path
//! rather than a public re-implementation of it.
//!
//! Every helper that CC1 already owns — provenance, the banded §6.2 linear
//! gate, the monitor code metric, the evidence artefact writer — is reused
//! from [`crate::cc1_fixtures`] rather than duplicated, so a CC1 tolerance can
//! never drift away from the CC3 fixture that claims to reuse it.
//!
//! Per CC3 §10.1.1 no expected value in this file is obtained by calling
//! `ColorWheels::apply`, `ColorCurve::evaluate`, the compositor, or the
//! shader. Expected values are either literal constants transcribed from the
//! contract or computed by the `spec_*_f64` functions below, which are an
//! independent f64 transcription of the §2 equations.

#![allow(clippy::cast_possible_truncation)]
#![allow(clippy::cast_possible_wrap)]
#![allow(clippy::cast_precision_loss)]
#![allow(clippy::cast_sign_loss)]
#![allow(clippy::excessive_precision)]
#![allow(clippy::float_cmp)]
#![allow(clippy::needless_pass_by_value)]
#![allow(clippy::similar_names)]
#![allow(clippy::too_many_arguments)]
#![allow(clippy::too_many_lines)]
#![allow(clippy::uninlined_format_args)]

use std::{collections::BTreeMap, sync::Arc};

use half::f16;
use kinewright_core::{
    Analysis, AssetId, AutomationCurve, COLOR_CURVE_COORDINATE_MAX, COLOR_CURVE_COORDINATE_MIN,
    COLOR_CURVE_MAX_POINTS, COLOR_NODE_LIMIT_PER_LAYER, ClipId, ColorCurveChannel,
    ColorNodeInactiveReason, ColorNodeKind, ColorSourceProfile, Command, Core, Document, Effect,
    EffectId, Event, JournalCommand, Keyframe, KeyframeInterpolation, OpError, Operation,
    ParamValue, ResolvedCurves, TimeCode, active_color_nodes, classify_color_node,
    color_node_inactive_reason, effect_descriptor,
};
use serde_json::{Value, json};

use crate::{
    Compositor, CompositorLayer,
    cc1_fixtures::{
        DiffMetrics, FixtureGpu, IDENTITY_RAMP_MONITOR_MAX, IDENTITY_RAMP_MONITOR_MEAN,
        IDENTITY_RAMP_MONITOR_P99, LINEAR_CPU_GPU_MAX, LINEAR_CPU_GPU_MEAN, LINEAR_CPU_GPU_P99,
        LINEAR_GATE_DOMAIN, LINEAR_GATE_IN_GAMUT, LINEAR_OVER_RANGE_MEAN, LINEAR_OVER_RANGE_P99,
        LinearParityMetrics, MIN_CHANGED_LINEAR_BASIS_POINTS, MONITOR_CPU_GPU_MAX,
        MONITOR_CPU_GPU_MEAN, MONITOR_CPU_GPU_P99, abs_code_diff_rgb, assert_linear_parity,
        assert_manifest_f32, assert_manifest_f64, backend_metadata, decode_managed_working_frame,
        fallback_gpu, file_hash, generate_delivery_source, git_revision, hardware_gpu,
        linear_parity_metrics, monitor_luma_and_clipping, output_hash, simple_document,
        working_frame, write_evidence_artefact,
    },
    color_pipeline::{
        ColorNode, apply_color_nodes_at, decode_bt709, encode_monitor_rgba8, grade709_decode,
        grade709_encode, resolve_color_nodes,
    },
    decode::probe_path,
    frame::WorkingFrame,
    initialize_ffmpeg,
    test_support::TempDirectory,
    timeline::TransitionRenderParams,
};

/// The contract token recorded on every CC3 evidence payload.
const CC3_CONTRACT: &str = "cc3_curves_and_wheels";

/// Non-GPU fixtures still record a backend so a reader never has to guess
/// which implementation produced a number.
const CPU_REFERENCE_BACKEND: &str = "backend=kinewright_media_cpu_reference;adapter=host_f32;\
software_fallback=true;gpu_claim=false;lane=cpu_reference";
const CPU_REFERENCE_LANE: &str = "cpu_reference";

/// Tolerance for the four §2.1 worked anchors. The contract states them as
/// normative to ±2e-5, so the fixture uses exactly that number.
const ANCHOR_TOLERANCE: f32 = 2.0e-5;

/// Relative + absolute tolerance when the production f32 node math is compared
/// against the independent f64 transcription of §2.
///
/// A per-control boundary case reaches `power = 4` and the `grade709` decode
/// exponent is `2.2222`, so one f32 ULP at the input is amplified by roughly
/// nine before it reaches the output. A flat 1e-6 would therefore be a coin
/// flip on the exponent boundaries rather than a contract check; 1e-5
/// relative with a 1e-7 absolute floor is still two orders of magnitude
/// tighter than the ±2e-5 the contract itself uses for its anchors.
const SPEC_RELATIVE_TOLERANCE: f64 = 1.0e-5;
const SPEC_ABSOLUTE_FLOOR: f64 = 1.0e-7;

/// The CC3 §10.2 raster: 24 linear levels crossed with 8 channel patterns.
pub(crate) const CC3_RASTER_LEVELS: [f32; 24] = [
    -0.50,
    -0.25,
    -0.10,
    -0.02,
    -0.005,
    0.0,
    0.002,
    0.005,
    0.018_053_969,
    0.03,
    0.06,
    0.10,
    0.18,
    0.25,
    0.35,
    0.50,
    0.65,
    0.80,
    0.90,
    1.00,
    1.20,
    1.50,
    2.50,
    4.00,
];

/// The eight §10.2 channel patterns, in raster order.
pub(crate) const CC3_PATTERNS: [&str; 8] = [
    "neutral", "red", "green", "blue", "cyan", "magenta", "yellow", "skewed",
];

/// Raster block width in pixels.
///
/// Wide blocks keep the production linear sampler on texel interiors, exactly
/// as the CC1 chart fixture does; a one-pixel-per-sample raster would measure
/// interpolated seams instead of the node math.
pub(crate) const CC3_RASTER_BLOCK_WIDTH: u32 = 8;
pub(crate) const CC3_RASTER_HEIGHT: u32 = 2;

/// The §10.2 sample count: 24 levels × 8 patterns.
const CC3_RASTER_SAMPLES: usize = CC3_RASTER_LEVELS.len() * CC3_PATTERNS.len();

/// The three §10.3.4 boundary samples: a negative, 18% grey, and over-range.
const BOUNDARY_SAMPLES: [f32; 3] = [-0.25, 0.18, 2.5];

/// Every evidence payload this suite emits. The manifest is asserted equal to
/// this list, so a fixture cannot be deleted without the manifest test failing.
const CC3_EVIDENCE_FIXTURES: [&str; 14] = [
    "cc3_parity_raster_coverage",
    "cc3_identity",
    "cc3_encoding_bijection",
    "cc3_monotonicity",
    "cc3_boundary_expected_values",
    "cc3_boundary_finiteness",
    "cc3_per_channel_independence",
    "cc3_collinear_identity",
    "cc3_node_ordering",
    "cc3_degenerate_automation",
    "cc3_gpu_cpu_parity",
    "cc3_serialization_history",
    "cc3_typed_rejections",
    "cc3_proof_parity",
];

// ---------------------------------------------------------------------------
// The §10.2 raster.
// ---------------------------------------------------------------------------

fn pattern_sample(pattern: usize, level: f32) -> [f32; 3] {
    match pattern {
        0 => [level, level, level],
        1 => [level, 0.0, 0.0],
        2 => [0.0, level, 0.0],
        3 => [0.0, 0.0, level],
        4 => [0.0, level, level],
        5 => [level, 0.0, level],
        6 => [level, level, 0.0],
        7 => [level, level / 2.0, level / 4.0],
        other => panic!("CC3 raster has eight patterns; asked for {other}"),
    }
}

/// The CC3 §10.2 parity raster: 192 RGB samples.
pub(crate) fn cc3_parity_raster() -> Vec<[f32; 3]> {
    let mut samples = Vec::with_capacity(CC3_RASTER_SAMPLES);
    for level in CC3_RASTER_LEVELS {
        for pattern in 0..CC3_PATTERNS.len() {
            samples.push(pattern_sample(pattern, level));
        }
    }
    assert_eq!(samples.len(), CC3_RASTER_SAMPLES);
    samples
}

/// The §10.2 raster as a wide-bar working frame.
pub(crate) fn cc3_raster_frame() -> (u32, u32, WorkingFrame) {
    let samples = cc3_parity_raster();
    let width = CC3_RASTER_BLOCK_WIDTH * samples.len() as u32;
    let height = CC3_RASTER_HEIGHT;
    let rgb = (0..width * height)
        .map(|index| samples[(index % width / CC3_RASTER_BLOCK_WIDTH) as usize])
        .collect::<Vec<_>>();
    (width, height, working_frame(width, height, &rgb))
}

/// A small frame holding only the three §10.3.4 boundary samples plus the
/// over-range extreme the §4.1 finiteness statement names explicitly.
fn boundary_frame() -> (u32, u32, WorkingFrame) {
    let samples: Vec<[f32; 3]> = BOUNDARY_SAMPLES
        .into_iter()
        .chain(std::iter::once(4.0))
        .map(|level| [level, level, level])
        .collect();
    let width = CC3_RASTER_BLOCK_WIDTH * samples.len() as u32;
    let height = CC3_RASTER_HEIGHT;
    let rgb = (0..width * height)
        .map(|index| samples[(index % width / CC3_RASTER_BLOCK_WIDTH) as usize])
        .collect::<Vec<_>>();
    (width, height, working_frame(width, height, &rgb))
}

/// Every distinct linear channel value the raster actually contains, sorted.
fn raster_channel_values() -> Vec<f32> {
    let mut values = cc3_parity_raster()
        .into_iter()
        .flatten()
        .collect::<Vec<f32>>();
    values.sort_by(f32::total_cmp);
    values.dedup();
    values
}

// ---------------------------------------------------------------------------
// Effect construction.
// ---------------------------------------------------------------------------

fn color_node_effect(id: u64, name: &str, parameters: BTreeMap<String, ParamValue>) -> Effect {
    Effect {
        id: EffectId(id),
        name: name.to_owned(),
        parameters,
        keyframes: BTreeMap::new(),
    }
}

fn wheels_effect(id: u64, parameters: &[(&str, i64)]) -> Effect {
    color_node_effect(
        id,
        "color_wheels",
        parameters
            .iter()
            .map(|(name, value)| ((*name).to_owned(), ParamValue::Integer(*value)))
            .collect(),
    )
}

/// The `{curve}_point_count` plus coordinate parameters for one curve.
fn curve_parameters(curve: ColorCurveChannel, points: &[(i64, i64)]) -> Vec<(String, i64)> {
    assert!(points.len() >= 2 && points.len() <= COLOR_CURVE_MAX_POINTS);
    let mut parameters = vec![(
        curve.point_count_parameter().to_owned(),
        points.len() as i64,
    )];
    for (index, (x, y)) in points.iter().enumerate() {
        parameters.push((
            curve
                .x_parameter(index)
                .expect("curve point index within bounds")
                .to_owned(),
            *x,
        ));
        parameters.push((
            curve
                .y_parameter(index)
                .expect("curve point index within bounds")
                .to_owned(),
            *y,
        ));
    }
    parameters
}

fn curves_effect(id: u64, curves: &[(ColorCurveChannel, &[(i64, i64)])]) -> Effect {
    let mut parameters = BTreeMap::new();
    for (curve, points) in curves {
        for (name, value) in curve_parameters(*curve, points) {
            parameters.insert(name, ParamValue::Integer(value));
        }
    }
    color_node_effect(id, "color_curves", parameters)
}

fn with_parameter(effect: &Effect, name: &str, value: i64) -> Effect {
    let mut updated = effect.clone();
    updated
        .parameters
        .insert(name.to_owned(), ParamValue::Integer(value));
    updated
}

/// A representative non-neutral wheels node used by the ordering, parity, and
/// proof fixtures.
pub(crate) fn representative_wheels(id: u64) -> Effect {
    wheels_effect(
        id,
        &[
            ("lift_master_basis_points", -250),
            ("lift_red_basis_points", 180),
            ("lift_blue_basis_points", -120),
            ("gamma_master_thousandths", 1_150),
            ("gamma_green_thousandths", 900),
            ("gain_master_thousandths", 1_050),
            ("gain_red_thousandths", 1_200),
            ("gain_blue_thousandths", 850),
        ],
    )
}

/// A representative non-neutral curves node: an S-curve on master, a lifted
/// red, and an over-range-aware blue.
pub(crate) fn representative_curves(id: u64) -> Effect {
    curves_effect(
        id,
        &[
            (
                ColorCurveChannel::Master,
                &[(0, 0), (2_500, 1_800), (7_500, 8_200), (10_000, 10_000)],
            ),
            (ColorCurveChannel::Red, &[(0, 400), (10_000, 9_600)]),
            (
                ColorCurveChannel::Blue,
                &[(-2_000, -2_000), (5_000, 4_600), (12_000, 12_000)],
            ),
        ],
    )
}

pub(crate) fn primary_effect(id: u64) -> Effect {
    color_node_effect(
        id,
        "primary_correction",
        [
            ("exposure_milli_stops", 750_i64),
            ("contrast_percent", 20),
            ("saturation_percent", 15),
            ("contrast_pivot_basis_points", 4_200),
        ]
        .into_iter()
        .map(|(name, value)| (name.to_owned(), ParamValue::Integer(value)))
        .collect(),
    )
}

// ---------------------------------------------------------------------------
// CPU reference and GPU rendering.
// ---------------------------------------------------------------------------

fn cpu_nodes(effects: &[Effect]) -> Vec<ColorNode> {
    resolve_color_nodes(effects).expect("CC3 fixture node stack must resolve")
}

/// The CC5 §3.4 pixel-centre uv of raster index `index`,
/// `((x + 0.5) / W, (y + 0.5) / H)`, matching the rasterizer's
/// `@builtin(position)` convention.
#[allow(clippy::cast_precision_loss)]
fn pixel_centre_uv(frame: &WorkingFrame, index: usize) -> [f32; 2] {
    let width = (frame.width.max(1)) as usize;
    let x = index % width;
    let y = index / width;
    [
        (x as f32 + 0.5) / frame.width.max(1) as f32,
        (y as f32 + 0.5) / frame.height.max(1) as f32,
    ]
}

/// The output raster aspect `a = W / H` the host supplies to the matte
/// (CC5 §3.2).
#[allow(clippy::cast_precision_loss)]
fn raster_aspect(frame: &WorkingFrame) -> f32 {
    frame.width.max(1) as f32 / frame.height.max(1) as f32
}

/// The CC5 §3.4 reference at the centre of a square raster.
///
/// No CC3 node stack carries a matte, so the position and the aspect are
/// immaterial and the result is bit-identical to the pre-CC5 positionless
/// reference — which is the point of CC5 §2.5's mandatory matte-free branch.
fn apply_stack(nodes: &[ColorNode], rgb: [f32; 3]) -> [f32; 3] {
    apply_color_nodes_at(nodes, rgb, [0.5, 0.5], 1.0)
}

/// The independent CPU reference in the linear working domain, including the
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
    resolution: (u32, u32),
    frame: &WorkingFrame,
    effects: &[Effect],
) -> Vec<f32> {
    compositor
        .render_working(
            resolution,
            &[CompositorLayer {
                frame,
                effects,
                transition: TransitionRenderParams::default(),
            }],
        )
        .expect("production GPU working-surface readback")
}

fn gpu_monitor(
    compositor: &Compositor,
    resolution: (u32, u32),
    frame: &WorkingFrame,
    effects: &[Effect],
) -> Vec<u8> {
    compositor
        .render(
            resolution,
            &[CompositorLayer {
                frame,
                effects,
                transition: TransitionRenderParams::default(),
            }],
        )
        .expect("production GPU compositor should render the CC3 fixture")
        .rgba
        .as_ref()
        .clone()
}

/// Fail a case whose CPU reference is indistinguishable from the baseline.
///
/// The CC1 helper takes a `PrimaryCorrection` for its failure message and so
/// cannot describe a wheels or curves case; the gate itself is CC1's
/// [`MIN_CHANGED_LINEAR_BASIS_POINTS`], reused rather than restated.
pub(crate) fn assert_case_is_not_vacuous(expected: &[f32], baseline: &[f32], label: &str) {
    assert_eq!(expected.len(), baseline.len());
    let compared = u64::try_from(expected.len() / 4 * 3).unwrap_or(0);
    let changed = expected
        .as_chunks::<4>()
        .0
        .iter()
        .zip(baseline.as_chunks::<4>().0.iter())
        .map(|(actual, baseline)| {
            actual[..3]
                .iter()
                .zip(&baseline[..3])
                .filter(|(actual, baseline)| actual != baseline)
                .count() as u64
        })
        .sum::<u64>();
    assert!(
        changed * 10_000 >= compared * MIN_CHANGED_LINEAR_BASIS_POINTS,
        "case {label} changed only {changed} of {compared} linear working RGB samples against the baseline; a non-neutral CC3 node must move the fixture raster or the case proves nothing"
    );
}

/// Render one case on the GPU, compare it against the independent CPU
/// reference, and apply the CC1 §6.2 gates verbatim.
fn assert_gpu_case(
    compositor: &Compositor,
    resolution: (u32, u32),
    frame: &WorkingFrame,
    effects: &[Effect],
    baseline_linear: Option<&[f32]>,
    label: &str,
) -> (DiffMetrics, LinearParityMetrics, Vec<u8>) {
    let nodes = cpu_nodes(effects);
    let expected_linear = cpu_reference_linear(frame, &nodes);
    let expected_monitor = cpu_reference_monitor(frame, &nodes);
    if let Some(baseline) = baseline_linear {
        assert_case_is_not_vacuous(&expected_linear, baseline, label);
    }
    let actual_linear = gpu_linear(compositor, resolution, frame, effects);
    let actual_monitor = gpu_monitor(compositor, resolution, frame, effects);
    let linear = linear_parity_metrics(&actual_linear, &expected_linear);
    let monitor = abs_code_diff_rgb(&actual_monitor, &expected_monitor);
    assert!(
        linear.in_gamut_samples > 0,
        "case {label} left the in-gamut §6.2 band empty, so the linear gate was never applied: {linear:?}"
    );
    assert!(
        monitor.max <= MONITOR_CPU_GPU_MAX,
        "GPU/CPU monitor max for {label}: {monitor:?}"
    );
    assert!(
        monitor.p99 <= MONITOR_CPU_GPU_P99,
        "GPU/CPU monitor P99 for {label}: {monitor:?}"
    );
    assert!(
        monitor.mean <= MONITOR_CPU_GPU_MEAN,
        "GPU/CPU monitor mean for {label}: {monitor:?}"
    );
    assert_linear_parity(&linear, label);
    (monitor, linear, actual_monitor)
}

// ---------------------------------------------------------------------------
// Evidence.
// ---------------------------------------------------------------------------

fn emit_cc3_evidence(
    fixture: &str,
    backend: &str,
    lane: &str,
    controls: Value,
    raster: (u32, u32),
    output_hash: String,
    metrics: Value,
) {
    assert!(
        CC3_EVIDENCE_FIXTURES.contains(&fixture),
        "every CC3 evidence payload must be declared in CC3_EVIDENCE_FIXTURES and in the manifest; {fixture} is not"
    );
    let provenance = backend_metadata(backend);
    let field = |key: &str| provenance.get(key).cloned().unwrap_or(Value::Null);
    let payload = json!({
        "contract": CC3_CONTRACT,
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
    println!("CC3_EVIDENCE {payload}");
    write_evidence_artefact(fixture, &payload);
}

pub(crate) fn json_hash(value: &Value) -> String {
    output_hash(
        serde_json::to_string(value)
            .expect("CC3 evidence must serialize")
            .as_bytes(),
    )
}

// ---------------------------------------------------------------------------
// The independent f64 transcription of CC3 §2 (fixture-quality rule 10.1.1).
//
// Nothing below calls the production crate. The constants are the §2.1 digits
// and the algorithms are the §2.2/§2.3 pseudocode, transcribed by hand, so a
// parity or boundary assertion compares two implementations of the written
// contract rather than one implementation with itself.
// ---------------------------------------------------------------------------

const SPEC_ALPHA: f64 = 1.099_296_8;
const SPEC_BETA: f64 = 0.018_053_969;
const SPEC_BETA_ENCODED: f64 = 0.081_242_86;
const SPEC_K: f64 = 0.099_296_8;
const SPEC_INVERSE_EXPONENT: f64 = 2.222_222_3;

/// `sgn(0) = 0`; `f64::signum` returns ±1 at zero and must not be used.
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
    if magnitude < SPEC_BETA {
        sign * 4.5 * magnitude
    } else {
        sign * (SPEC_ALPHA * magnitude.powf(0.45) - SPEC_K)
    }
}

fn spec_grade709_decode_f64(e: f64) -> f64 {
    let sign = spec_sign_f64(e);
    let magnitude = e.abs();
    if magnitude < SPEC_BETA_ENCODED {
        sign * magnitude / 4.5
    } else {
        sign * ((magnitude + SPEC_K) / SPEC_ALPHA).powf(SPEC_INVERSE_EXPONENT)
    }
}

fn spec_control(parameters: &[(&str, i64)], name: &str, neutral: i64) -> i64 {
    parameters
        .iter()
        .find(|(stored, _)| *stored == name)
        .map_or(neutral, |(_, value)| *value)
}

/// CC3 §2.2, transcribed: slope/offset/power per channel in `grade709`.
fn spec_wheels_apply_f64(parameters: &[(&str, i64)], rgb: [f64; 3]) -> [f64; 3] {
    const CHANNELS: [&str; 3] = ["red", "green", "blue"];
    let lift_master = spec_control(parameters, "lift_master_basis_points", 0);
    let gamma_master = spec_control(parameters, "gamma_master_thousandths", 1_000);
    let gain_master = spec_control(parameters, "gain_master_thousandths", 1_000);
    std::array::from_fn(|channel| {
        let name = CHANNELS[channel];
        let lift = spec_control(parameters, &format!("lift_{name}_basis_points"), 0);
        let gamma = spec_control(parameters, &format!("gamma_{name}_thousandths"), 1_000);
        let gain = spec_control(parameters, &format!("gain_{name}_thousandths"), 1_000);
        let slope = (gain as f64 / 1_000.0) * (gain_master as f64 / 1_000.0);
        let offset = (lift + lift_master) as f64 / 10_000.0;
        let power = (gamma as f64 / 1_000.0) * (gamma_master as f64 / 1_000.0);
        let y = spec_grade709_encode_f64(rgb[channel]) * slope + offset;
        let z = spec_sign_f64(y) * y.abs().powf(power);
        spec_grade709_decode_f64(z)
    })
}

/// CC3 §2.3, transcribed: Fritsch--Carlson tangents plus Hermite evaluation.
#[derive(Debug, Clone)]
struct SpecCurve {
    xs: Vec<f64>,
    ys: Vec<f64>,
    tangents: Vec<f64>,
}

impl SpecCurve {
    fn new(points: &[(i64, i64)]) -> Self {
        assert!(points.len() >= 2, "a CC3 curve holds at least two points");
        let xs: Vec<f64> = points.iter().map(|(x, _)| *x as f64 / 10_000.0).collect();
        let ys: Vec<f64> = points.iter().map(|(_, y)| *y as f64 / 10_000.0).collect();
        let count = points.len();

        // Step 1.
        let deltas: Vec<f64> = (0..count - 1)
            .map(|i| (ys[i + 1] - ys[i]) / (xs[i + 1] - xs[i]))
            .collect();

        // Step 2.
        let mut tangents = vec![0.0_f64; count];
        tangents[0] = deltas[0];
        tangents[count - 1] = deltas[count - 2];
        // The contract writes the interior tangent as a literal average.
        // `f64::midpoint` takes a different branch for huge magnitudes, and a
        // transcription of the contract must not carry a second rounding rule.
        #[allow(clippy::manual_midpoint)]
        for i in 1..count - 1 {
            tangents[i] = (deltas[i - 1] + deltas[i]) / 2.0;
        }

        // Step 3: forward, in place. The visitation order is normative.
        for (i, delta) in deltas.iter().copied().enumerate() {
            if delta == 0.0 {
                tangents[i] = 0.0;
                tangents[i + 1] = 0.0;
                continue;
            }
            let a = tangents[i] / delta;
            let b = tangents[i + 1] / delta;
            if a < 0.0 {
                tangents[i] = 0.0;
            }
            if b < 0.0 {
                tangents[i + 1] = 0.0;
            }
            if a >= 0.0 && b >= 0.0 && a * a + b * b > 9.0 {
                let tau = 3.0 / (a * a + b * b).sqrt();
                tangents[i] = tau * a * delta;
                tangents[i + 1] = tau * b * delta;
            }
        }
        Self { xs, ys, tangents }
    }

    fn identity() -> Self {
        Self::new(&[(0, 0), (10_000, 10_000)])
    }

    fn evaluate(&self, x: f64) -> f64 {
        let last = self.xs.len() - 1;
        if x < self.xs[0] {
            return self.ys[0] + self.tangents[0] * (x - self.xs[0]);
        }
        if x >= self.xs[last] {
            return self.ys[last] + self.tangents[last] * (x - self.xs[last]);
        }
        let mut segment = 0;
        for index in 0..last {
            if x >= self.xs[index] && x < self.xs[index + 1] {
                segment = index;
            }
        }
        let (x0, y0, m0) = (self.xs[segment], self.ys[segment], self.tangents[segment]);
        let (x1, y1, m1) = (
            self.xs[segment + 1],
            self.ys[segment + 1],
            self.tangents[segment + 1],
        );
        let h = x1 - x0;
        let t = (x - x0) / h;
        let t2 = t * t;
        let t3 = t2 * t;
        (2.0 * t3 - 3.0 * t2 + 1.0) * y0
            + (t3 - 2.0 * t2 + t) * h * m0
            + (-2.0 * t3 + 3.0 * t2) * y1
            + (t3 - t2) * h * m1
    }
}

/// CC3 §2.3 node evaluation: per-channel curves first, then master.
fn spec_curves_apply_f64(curves: &[(ColorCurveChannel, &[(i64, i64)])], rgb: [f64; 3]) -> [f64; 3] {
    let solved = |channel: ColorCurveChannel| {
        curves
            .iter()
            .find(|(stored, _)| *stored == channel)
            .map_or_else(SpecCurve::identity, |(_, points)| SpecCurve::new(points))
    };
    let red = solved(ColorCurveChannel::Red);
    let green = solved(ColorCurveChannel::Green);
    let blue = solved(ColorCurveChannel::Blue);
    let master = solved(ColorCurveChannel::Master);
    let encoded = [
        red.evaluate(spec_grade709_encode_f64(rgb[0])),
        green.evaluate(spec_grade709_encode_f64(rgb[1])),
        blue.evaluate(spec_grade709_encode_f64(rgb[2])),
    ];
    encoded.map(|value| spec_grade709_decode_f64(master.evaluate(value)))
}

fn spec_tolerance(expected: f64) -> f64 {
    SPEC_RELATIVE_TOLERANCE * expected.abs() + SPEC_ABSOLUTE_FLOOR
}

fn assert_matches_spec(actual: f32, expected: f64, label: &str) {
    let tolerance = spec_tolerance(expected);
    assert!(
        (f64::from(actual) - expected).abs() <= tolerance,
        "{label}: production f32 {actual} does not match the hand-derived f64 spec value {expected} within {tolerance}"
    );
}

fn assert_close(actual: f32, expected: f32, tolerance: f32, label: &str) {
    assert!(
        (actual - expected).abs() <= tolerance,
        "{label}: {actual} is not within {tolerance} of {expected}"
    );
}

// ---------------------------------------------------------------------------
// §10.2 / §10.3.1: the raster and its coverage, and the identity gate.
// ---------------------------------------------------------------------------

/// CC3 §10.2. The raster asserts its own coverage; a raster that fails
/// coverage fails the suite.
///
/// Counting rule, stated because the contract's numbers only close under one
/// reading: the §10.2 counts are applied to the **distinct linear channel
/// values the 192-sample raster actually contains**, not to the 24-entry level
/// list. The level list alone holds four levels above 1.0 and four in
/// `(0.5, 1.0]`; it is the skewed `(L, L/2, L/4)` pattern that lifts those to
/// the documented six and (with `0.5` included) eight. The fixture records the
/// strict `(0.5, 1.0]` count separately and asserts it at its true value of
/// seven rather than claiming coverage the raster does not have.
#[test]
fn cc3_parity_raster_asserts_its_own_documented_coverage() {
    // Transcribed a second time from §10.2 so a typo in CC3_RASTER_LEVELS
    // cannot silently redefine the contract raster.
    const DOCUMENTED_LEVELS: [f32; 24] = [
        -0.50,
        -0.25,
        -0.10,
        -0.02,
        -0.005,
        0.0,
        0.002,
        0.005,
        0.018_053_969,
        0.03,
        0.06,
        0.10,
        0.18,
        0.25,
        0.35,
        0.50,
        0.65,
        0.80,
        0.90,
        1.00,
        1.20,
        1.50,
        2.50,
        4.00,
    ];
    assert_eq!(CC3_RASTER_LEVELS, DOCUMENTED_LEVELS);
    assert_eq!(CC3_PATTERNS.len(), 8);

    let raster = cc3_parity_raster();
    assert_eq!(raster.len(), 192, "24 levels x 8 patterns");
    assert_eq!(raster.len(), CC3_RASTER_SAMPLES);

    let values = raster_channel_values();
    let minimum = values.first().copied().expect("raster is not empty");
    let maximum = values.last().copied().expect("raster is not empty");
    let negatives = values.iter().filter(|value| **value < 0.0).count();
    let above_one = values.iter().filter(|value| **value > 1.0).count();
    let upper_mid_open = values
        .iter()
        .filter(|value| **value > 0.5 && **value <= 1.0)
        .count();
    let upper_mid_closed = values
        .iter()
        .filter(|value| **value >= 0.5 && **value <= 1.0)
        .count();

    assert!(minimum <= -0.25, "minimum raster level {minimum} > -0.25");
    assert!(maximum >= 4.0, "maximum raster level {maximum} < 4.0");
    assert!(
        negatives >= 5,
        "raster has only {negatives} negative levels"
    );
    assert!(
        above_one >= 6,
        "raster has only {above_one} distinct levels above 1.0"
    );
    assert!(
        upper_mid_closed >= 8,
        "raster has only {upper_mid_closed} distinct levels in [0.5, 1.0]"
    );
    assert!(
        upper_mid_open >= 7,
        "raster has only {upper_mid_open} distinct levels in (0.5, 1.0]"
    );

    // §10.1.3: the CC1 raster's failure mode was that no sample exceeded 0.2
    // linear. Assert the span explicitly rather than trusting the level list.
    assert!(
        values.iter().any(|value| *value >= 4.0),
        "raster must reach 4.0 linear"
    );
    assert!(
        values
            .iter()
            .any(|value| (*value - 0.018_053_969).abs() < 1e-9),
        "raster must contain the grade709 breakpoint itself"
    );
    for (index, pattern) in CC3_PATTERNS.into_iter().enumerate() {
        let sample = pattern_sample(index, 0.8);
        assert!(
            raster.contains(&sample),
            "raster is missing the {pattern} pattern at level 0.8"
        );
    }

    let (width, height, frame) = cc3_raster_frame();
    assert_eq!(width, 192 * CC3_RASTER_BLOCK_WIDTH);
    assert_eq!(height, CC3_RASTER_HEIGHT);
    assert_eq!(frame.pixels.len(), (width * height * 4) as usize);

    let coverage = json!({
        "levels": CC3_RASTER_LEVELS,
        "patterns": CC3_PATTERNS,
        "rgb_samples": raster.len(),
        "distinct_channel_values": values.len(),
        "minimum_level": minimum,
        "maximum_level": maximum,
        "negative_levels": negatives,
        "levels_above_one": above_one,
        "levels_in_half_open_upper_mid": upper_mid_open,
        "levels_in_closed_upper_mid": upper_mid_closed,
        "counting_domain": "distinct linear channel values present in the 192-sample raster",
    });
    emit_cc3_evidence(
        "cc3_parity_raster_coverage",
        CPU_REFERENCE_BACKEND,
        CPU_REFERENCE_LANE,
        json!({"raster": "cc3_parity_raster", "block_width": CC3_RASTER_BLOCK_WIDTH}),
        (width, height),
        json_hash(&coverage),
        coverage,
    );
}

pub(crate) fn bits_of(values: &[f32]) -> Vec<u32> {
    values.iter().map(|value| value.to_bits()).collect()
}

/// CC3 §10.3.1. Neutral and bypassed nodes are the exact identity: they are
/// never written to the GPU buffer and never evaluated on the CPU, so the
/// result is bit-identical to the same stack with the node removed.
#[test]
fn cc3_inactive_nodes_are_bit_identical_to_the_stack_without_them() {
    let gpu = fallback_gpu();
    let compositor = Compositor::new(gpu.context());
    let (width, height, frame) = cc3_raster_frame();
    let resolution = (width, height);

    let empty_buffer =
        crate::compositor::grade_buffer_bytes(&[]).expect("an empty stack must serialize");
    assert_eq!(
        empty_buffer.len(),
        80,
        "CC3 §3.2: an empty node stack is a 16-byte header plus one zeroed record"
    );

    let structural_identity: [(i64, i64); 2] = [(0, 0), (10_000, 10_000)];
    let cases: Vec<(&str, Effect, ColorNodeInactiveReason)> = vec![
        (
            "neutral_wheels_by_omission",
            wheels_effect(1, &[]),
            ColorNodeInactiveReason::Neutral,
        ),
        (
            "neutral_wheels_written_out",
            wheels_effect(
                2,
                &[
                    ("lift_master_basis_points", 0),
                    ("lift_red_basis_points", 0),
                    ("lift_green_basis_points", 0),
                    ("lift_blue_basis_points", 0),
                    ("gamma_master_thousandths", 1_000),
                    ("gamma_red_thousandths", 1_000),
                    ("gamma_green_thousandths", 1_000),
                    ("gamma_blue_thousandths", 1_000),
                    ("gain_master_thousandths", 1_000),
                    ("gain_red_thousandths", 1_000),
                    ("gain_green_thousandths", 1_000),
                    ("gain_blue_thousandths", 1_000),
                    ("bypass", 0),
                ],
            ),
            ColorNodeInactiveReason::Neutral,
        ),
        (
            "structural_identity_curves",
            curves_effect(
                3,
                &[
                    (ColorCurveChannel::Master, &structural_identity),
                    (ColorCurveChannel::Red, &structural_identity),
                    (ColorCurveChannel::Green, &structural_identity),
                    (ColorCurveChannel::Blue, &structural_identity),
                ],
            ),
            ColorNodeInactiveReason::Neutral,
        ),
        (
            "bypassed_non_neutral_wheels",
            with_parameter(&representative_wheels(4), "bypass", 1),
            ColorNodeInactiveReason::Bypassed,
        ),
        (
            "bypassed_non_neutral_curves",
            with_parameter(&representative_curves(5), "bypass", 1),
            ColorNodeInactiveReason::Bypassed,
        ),
    ];

    // Two carriers: the node alone, and the node behind an active primary so a
    // non-empty buffer cannot hide a mis-serialized inactive record.
    let primary = primary_effect(90);
    let carriers: [(&str, Vec<Effect>); 2] = [
        ("alone", Vec::new()),
        ("after_primary", vec![primary.clone()]),
    ];

    let mut recorded = Vec::new();
    for (carrier_name, carrier) in carriers {
        let baseline_cpu = cpu_reference_linear(&frame, &cpu_nodes(&carrier));
        let baseline_linear = gpu_linear(&compositor, resolution, &frame, &carrier);
        let baseline_monitor = gpu_monitor(&compositor, resolution, &frame, &carrier);
        let baseline_buffer =
            crate::compositor::grade_buffer_bytes(&carrier).expect("carrier stack serializes");
        for (name, effect, reason) in &cases {
            assert_eq!(
                color_node_inactive_reason(effect),
                Some(*reason),
                "{name} must be inactive for the documented reason"
            );
            let mut stack = carrier.clone();
            stack.push(effect.clone());
            let nodes = cpu_nodes(&stack);
            assert_eq!(
                nodes.len(),
                carrier.len(),
                "{name}: CC3 §3.3 requires the inactive node to be skipped entirely"
            );
            assert_eq!(
                crate::compositor::grade_buffer_bytes(&stack).expect("inactive stack serializes"),
                baseline_buffer,
                "{name}: an inactive node must not reach the GPU buffer"
            );

            let cpu = cpu_reference_linear(&frame, &nodes);
            assert_eq!(
                bits_of(&cpu),
                bits_of(&baseline_cpu),
                "{name} ({carrier_name}): CPU linear working values must be bit-identical"
            );
            let linear = gpu_linear(&compositor, resolution, &frame, &stack);
            assert_eq!(
                bits_of(&linear),
                bits_of(&baseline_linear),
                "{name} ({carrier_name}): GPU linear working values must be bit-identical"
            );
            let monitor = gpu_monitor(&compositor, resolution, &frame, &stack);
            assert_eq!(
                monitor, baseline_monitor,
                "{name} ({carrier_name}): monitor RGBA8 must be bit-identical"
            );
            recorded.push(json!({
                "case": name,
                "carrier": carrier_name,
                "inactive_reason": reason.as_str(),
                "cpu_linear_bit_identical": true,
                "gpu_linear_bit_identical": true,
                "monitor_rgba8_bit_identical": true,
                "grade_buffer_bytes": baseline_buffer.len(),
            }));
        }
    }

    let metrics = json!({
        "lane": gpu.lane.id(),
        "empty_stack_bytes": empty_buffer.len(),
        "cases": recorded,
    });
    emit_cc3_evidence(
        "cc3_identity",
        gpu.backend(),
        gpu.lane.id(),
        json!({"nodes": ["color_wheels", "color_curves"], "reasons": ["neutral", "bypassed"]}),
        resolution,
        json_hash(&metrics),
        metrics,
    );
}

// ---------------------------------------------------------------------------
// §10.3.2: the encoding bijection.
// ---------------------------------------------------------------------------

/// CC3 §10.3.2. `D(E(x)) == x` within `LINEAR_CPU_GPU_MAX` over the raster,
/// `E` is strictly increasing, and the four §2.1 anchors hold to ±2e-5.
#[test]
fn cc3_grade709_is_a_bijection_and_matches_the_documented_anchors() {
    let values = raster_channel_values();
    let mut worst_round_trip = 0.0_f32;
    let mut previous_encoded = f32::NEG_INFINITY;
    let mut previous_value = f32::NEG_INFINITY;
    for value in &values {
        let encoded = grade709_encode(*value);
        assert!(
            encoded > previous_encoded,
            "E is not strictly increasing: E({previous_value}) = {previous_encoded} >= E({value}) = {encoded}"
        );
        previous_encoded = encoded;
        previous_value = *value;
        let round_trip = (grade709_decode(encoded) - *value).abs();
        assert!(
            round_trip <= LINEAR_CPU_GPU_MAX,
            "D(E({value})) missed by {round_trip}, above LINEAR_CPU_GPU_MAX"
        );
        worst_round_trip = worst_round_trip.max(round_trip);
    }

    // CC3 §2.1 worked anchors, written as literals. The contract states them
    // as normative to ±2e-5.
    assert_close(
        grade709_encode(0.18),
        0.408_848,
        ANCHOR_TOLERANCE,
        "E(0.18)",
    );
    let gain_anchor = wheels_effect(1, &[("gain_red_thousandths", 1_200)]);
    assert_close(
        apply_stack(&cpu_nodes(std::slice::from_ref(&gain_anchor)), [0.18; 3])[0],
        0.250_771,
        ANCHOR_TOLERANCE,
        "wheels gain_red = 1200 at 0.18",
    );
    let lift_gamma_anchor = wheels_effect(
        2,
        &[
            ("lift_master_basis_points", -500),
            ("gamma_master_thousandths", 1_200),
        ],
    );
    assert_close(
        apply_stack(
            &cpu_nodes(std::slice::from_ref(&lift_gamma_anchor)),
            [0.18; 3],
        )[0],
        0.100_923,
        ANCHOR_TOLERANCE,
        "wheels lift_master = -500, gamma_master = 1200 at 0.18",
    );
    let curve_anchor = curves_effect(
        3,
        &[(
            ColorCurveChannel::Master,
            &[(0, 0), (5_000, 6_000), (10_000, 10_000)],
        )],
    );
    assert_close(
        apply_stack(&cpu_nodes(std::slice::from_ref(&curve_anchor)), [0.18; 3])[0],
        0.262_441,
        ANCHOR_TOLERANCE,
        "curves master (0,0) (5000,6000) (10000,10000) at 0.18",
    );

    // E(0) = 0 and D(0) = 0 exactly: `sgn(0) = 0`, which is what makes the
    // §10.3.1 identity gate bit-identical rather than tolerance-bounded.
    assert_eq!(grade709_encode(0.0).to_bits(), 0.0_f32.to_bits());
    assert_eq!(grade709_decode(0.0).to_bits(), 0.0_f32.to_bits());
    assert_close(grade709_encode(1.0), 1.0, ANCHOR_TOLERANCE, "E(1)");
    assert_close(grade709_decode(1.0), 1.0, ANCHOR_TOLERANCE, "D(1)");

    // The independent f64 transcription must agree with the production f32
    // pair over the whole raster, or one of the two is wrong.
    let mut worst_spec = 0.0_f64;
    for value in &values {
        let expected = spec_grade709_encode_f64(f64::from(*value));
        assert_matches_spec(grade709_encode(*value), expected, &format!("E({value})"));
        worst_spec = worst_spec.max((f64::from(grade709_encode(*value)) - expected).abs());
    }

    let metrics = json!({
        "raster_channel_values": values.len(),
        "max_round_trip_error": worst_round_trip,
        "round_trip_gate": LINEAR_CPU_GPU_MAX,
        "strictly_increasing": true,
        "max_spec_f64_deviation": worst_spec,
        "anchors": {
            "encode_0_18": 0.408_848,
            "wheels_gain_red_1200_at_0_18": 0.250_771,
            "wheels_lift_master_minus_500_gamma_master_1200_at_0_18": 0.100_923,
            "curves_master_0_5000_6000_10000_at_0_18": 0.262_441,
            "tolerance": ANCHOR_TOLERANCE,
        },
    });
    emit_cc3_evidence(
        "cc3_encoding_bijection",
        CPU_REFERENCE_BACKEND,
        CPU_REFERENCE_LANE,
        json!({"encoding": "grade709"}),
        (0, 0),
        json_hash(&metrics),
        metrics,
    );
}

// ---------------------------------------------------------------------------
// §10.3.3: monotonicity.
// ---------------------------------------------------------------------------

/// A neutral BT.709 ramp of `2^depth_bits` codes, decoded to scene-linear.
pub(crate) fn neutral_ramp(depth_bits: u32) -> (u32, u32, WorkingFrame) {
    let levels = 1_u32 << depth_bits;
    let maximum = (levels - 1) as f32;
    let rgb = (0..levels)
        .map(|code| {
            let linear = decode_bt709(code as f32 / maximum);
            [linear, linear, linear]
        })
        .collect::<Vec<_>>();
    (levels, 1, working_frame(levels, 1, &rgb))
}

/// Adjacent RGB pairs that descend left to right after monitor encoding.
pub(crate) fn descending_pairs(monitor: &[u8], width: u32, height: u32) -> usize {
    let pixels = monitor.as_chunks::<4>().0;
    assert_eq!(pixels.len(), (width * height) as usize);
    let mut descending = 0;
    for row in 0..height as usize {
        let start = row * width as usize;
        for column in 1..width as usize {
            let previous = pixels[start + column - 1];
            let current = pixels[start + column];
            for channel in 0..3 {
                if current[channel] < previous[channel] {
                    descending += 1;
                }
            }
        }
    }
    descending
}

/// A 16-point monotone curve: strictly increasing `x`, non-decreasing `y`.
const SIXTEEN_POINT_MONOTONE: [(i64, i64); 16] = [
    (0, 0),
    (667, 400),
    (1_333, 900),
    (2_000, 1_500),
    (2_667, 2_200),
    (3_333, 3_000),
    (4_000, 3_900),
    (4_667, 4_800),
    (5_333, 5_700),
    (6_000, 6_600),
    (6_667, 7_400),
    (7_333, 8_100),
    (8_000, 8_700),
    (8_667, 9_200),
    (9_333, 9_600),
    (10_000, 10_000),
];

/// A monotone point sequence with an exact zero-slope plateau.
const PLATEAU_CURVE: [(i64, i64); 4] = [(0, 0), (2_500, 3_000), (5_000, 3_000), (10_000, 10_000)];

/// CC3 §10.3.3. Monotone-point curves and every wheels boundary combination
/// with `slope > 0` produce zero descending adjacent pairs on the 8-bit and
/// 10-bit neutral ramps after final monitor encoding.
#[test]
fn cc3_monotone_nodes_never_descend_on_the_neutral_ramps() {
    // `gain = 0` is excluded here by construction: §2.2 makes a zero slope a
    // legal constant channel, which is monotone non-decreasing but is covered
    // by the §10.3.4 boundary fixture instead.
    const LIFT_VALUES: [i64; 3] = [-2_000, 0, 2_000];
    const GAMMA_VALUES: [i64; 3] = [100, 1_000, 4_000];
    const GAIN_VALUES: [i64; 3] = [1, 1_000, 4_000];

    let gpu = fallback_gpu();
    let compositor = Compositor::new(gpu.context());
    let ramps = [("8_bit", neutral_ramp(8)), ("10_bit", neutral_ramp(10))];

    let curve_cases: Vec<(&str, Effect)> = vec![
        (
            "monotone_master_curve",
            curves_effect(
                1,
                &[(
                    ColorCurveChannel::Master,
                    &[(0, 500), (3_000, 2_000), (7_000, 9_000), (10_000, 9_800)],
                )],
            ),
        ),
        (
            "zero_slope_plateau",
            curves_effect(2, &[(ColorCurveChannel::Master, &PLATEAU_CURVE)]),
        ),
        (
            "sixteen_point_monotone",
            curves_effect(3, &[(ColorCurveChannel::Master, &SIXTEEN_POINT_MONOTONE)]),
        ),
        (
            "per_channel_monotone_with_plateau",
            curves_effect(
                4,
                &[
                    (ColorCurveChannel::Red, &SIXTEEN_POINT_MONOTONE),
                    (ColorCurveChannel::Green, &PLATEAU_CURVE),
                    (
                        ColorCurveChannel::Blue,
                        &[(-2_000, -2_000), (4_000, 5_000), (12_000, 12_000)],
                    ),
                ],
            ),
        ),
    ];

    let mut wheels_cases: Vec<(String, Effect)> = Vec::new();
    let mut identifier = 100_u64;
    for lift in LIFT_VALUES {
        for gamma in GAMMA_VALUES {
            for gain in GAIN_VALUES {
                identifier += 1;
                wheels_cases.push((
                    format!("master_lift{lift}_gamma{gamma}_gain{gain}"),
                    wheels_effect(
                        identifier,
                        &[
                            ("lift_master_basis_points", lift),
                            ("gamma_master_thousandths", gamma),
                            ("gain_master_thousandths", gain),
                        ],
                    ),
                ));
                identifier += 1;
                wheels_cases.push((
                    format!("per_channel_lift{lift}_gamma{gamma}_gain{gain}"),
                    wheels_effect(
                        identifier,
                        &[
                            ("lift_red_basis_points", lift),
                            ("lift_green_basis_points", lift),
                            ("lift_blue_basis_points", lift),
                            ("gamma_red_thousandths", gamma),
                            ("gamma_green_thousandths", gamma),
                            ("gamma_blue_thousandths", gamma),
                            ("gain_red_thousandths", gain),
                            ("gain_green_thousandths", gain),
                            ("gain_blue_thousandths", gain),
                        ],
                    ),
                ));
            }
        }
    }
    assert_eq!(
        wheels_cases.len(),
        LIFT_VALUES.len() * GAMMA_VALUES.len() * GAIN_VALUES.len() * 2,
        "every boundary combination with slope > 0 must be covered"
    );

    // A representative extreme subset also runs through the production shader
    // so the GPU dispatch is held to the same gate.
    let gpu_case_names = [
        "monotone_master_curve",
        "zero_slope_plateau",
        "sixteen_point_monotone",
        "per_channel_monotone_with_plateau",
        "master_lift-2000_gamma4000_gain4000",
        "master_lift2000_gamma100_gain1",
        "per_channel_lift2000_gamma4000_gain1",
        "per_channel_lift-2000_gamma100_gain4000",
    ];

    let mut checked = 0_usize;
    let mut gpu_checked = 0_usize;
    for (ramp_name, (width, height, frame)) in &ramps {
        for (name, effect) in curve_cases
            .iter()
            .map(|(name, effect)| ((*name).to_owned(), effect.clone()))
            .chain(wheels_cases.iter().cloned())
        {
            let stack = [effect.clone()];
            let nodes = cpu_nodes(&stack);
            let cpu = cpu_reference_monitor(frame, &nodes);
            let cpu_descending = descending_pairs(&cpu, *width, *height);
            assert_eq!(
                cpu_descending, 0,
                "CPU reference descended {cpu_descending} times for {name} on the {ramp_name} ramp"
            );
            checked += 1;
            if gpu_case_names.contains(&name.as_str()) {
                let rendered = gpu_monitor(&compositor, (*width, *height), frame, &stack);
                let gpu_descending = descending_pairs(&rendered, *width, *height);
                assert_eq!(
                    gpu_descending, 0,
                    "production shader descended {gpu_descending} times for {name} on the {ramp_name} ramp"
                );
                gpu_checked += 1;
            }
        }
    }

    let metrics = json!({
        "lane": gpu.lane.id(),
        "ramps": ["8_bit", "10_bit"],
        "ramp_codes": [256, 1_024],
        "curve_cases": curve_cases.iter().map(|(name, _)| *name).collect::<Vec<_>>(),
        "wheels_boundary_combinations": wheels_cases.len(),
        "cpu_case_evaluations": checked,
        "gpu_case_evaluations": gpu_checked,
        "descending_adjacent_pairs": 0,
    });
    emit_cc3_evidence(
        "cc3_monotonicity",
        gpu.backend(),
        gpu.lane.id(),
        json!({"zero_slope_plateau": PLATEAU_CURVE, "sixteen_point_curve": SIXTEEN_POINT_MONOTONE}),
        (1_024, 1),
        json_hash(&metrics),
        metrics,
    );
}

// ---------------------------------------------------------------------------
// §10.3.4: boundaries.
// ---------------------------------------------------------------------------

/// The §4.1 control table: name, minimum, maximum, neutral, interior probe.
///
/// The bounds are asserted against the registered descriptor below, and the
/// interior probe is asserted to be strictly inside the bounds and different
/// from the neutral, so no row can quietly become a no-op.
const WHEEL_CONTROLS: [(&str, i64, i64, i64, i64); 12] = [
    ("lift_master_basis_points", -2_000, 2_000, 0, 750),
    ("lift_red_basis_points", -2_000, 2_000, 0, -600),
    ("lift_green_basis_points", -2_000, 2_000, 0, 420),
    ("lift_blue_basis_points", -2_000, 2_000, 0, -1_100),
    ("gamma_master_thousandths", 100, 4_000, 1_000, 1_600),
    ("gamma_red_thousandths", 100, 4_000, 1_000, 700),
    ("gamma_green_thousandths", 100, 4_000, 1_000, 2_400),
    ("gamma_blue_thousandths", 100, 4_000, 1_000, 1_250),
    ("gain_master_thousandths", 0, 4_000, 1_000, 1_750),
    ("gain_red_thousandths", 0, 4_000, 1_000, 300),
    ("gain_green_thousandths", 0, 4_000, 1_000, 2_600),
    ("gain_blue_thousandths", 0, 4_000, 1_000, 1_400),
];

/// Curve configurations that place `point_count` and both coordinates at their
/// §4.2 minimum, maximum, and an interior value.
fn boundary_curve_cases() -> Vec<(&'static str, Vec<(i64, i64)>)> {
    let interior_eight = (0..8)
        .map(|index| {
            let x = index * 1_400 - 1_000;
            let y = index * 1_500 - 1_200;
            (x, y)
        })
        .collect::<Vec<_>>();
    vec![
        ("point_count_minimum_two", vec![(0, 500), (10_000, 9_500)]),
        ("point_count_interior_eight", interior_eight),
        (
            "point_count_maximum_sixteen",
            SIXTEEN_POINT_MONOTONE
                .into_iter()
                .map(|(x, y)| (x, y.min(9_900)))
                .collect(),
        ),
        (
            "coordinates_at_minimum",
            vec![(-2_000, -2_000), (0, 500), (10_000, 10_000)],
        ),
        (
            "coordinates_at_maximum",
            vec![(0, 0), (10_000, 9_000), (12_000, 12_000)],
        ),
        (
            "coordinates_span_both_bounds",
            vec![(-2_000, 12_000), (5_000, 5_000), (12_000, -2_000)],
        ),
        (
            "coordinate_interior",
            vec![(0, 0), (3_000, 4_200), (10_000, 10_000)],
        ),
    ]
}

/// CC3 §10.3.4, first half. Every §4 control at its minimum, maximum, and one
/// interior value has a written-out expected value on a negative sample, on
/// 0.18, and on an over-range sample. The expected value is computed by the
/// independent f64 transcription of §2, never by calling the crate.
#[test]
fn cc3_every_control_bound_matches_a_hand_derived_expected_value() {
    let wheels_descriptor =
        effect_descriptor("color_wheels").expect("color_wheels must be registered");
    let curves_descriptor =
        effect_descriptor("color_curves").expect("color_curves must be registered");
    let mut recorded = Vec::new();
    let mut checks = 0_usize;

    for (name, minimum, maximum, neutral, interior) in WHEEL_CONTROLS {
        let parameter = wheels_descriptor
            .parameter(name)
            .unwrap_or_else(|| panic!("{name} must be a registered color_wheels parameter"));
        assert_eq!(
            (parameter.min, parameter.max, parameter.neutral),
            (minimum, maximum, neutral),
            "{name} bounds drifted from the §4.1 table"
        );
        assert!(
            interior > minimum && interior < maximum && interior != neutral,
            "{name} interior probe {interior} must be strictly inside the bounds and non-neutral"
        );
        for (position, value) in [
            ("minimum", minimum),
            ("maximum", maximum),
            ("interior", interior),
        ] {
            let parameters = [(name, value)];
            let effect = wheels_effect(1, &parameters);
            let nodes = cpu_nodes(std::slice::from_ref(&effect));
            assert_eq!(
                nodes.len(),
                1,
                "{name}={value} is non-neutral and must stay active"
            );
            let mut expectations = Vec::new();
            for sample in BOUNDARY_SAMPLES {
                let expected = spec_wheels_apply_f64(&parameters, [f64::from(sample); 3]);
                let actual = apply_stack(&nodes, [sample; 3]);
                for channel in 0..3 {
                    assert_matches_spec(
                        actual[channel],
                        expected[channel],
                        &format!(
                            "color_wheels {name}={value} at linear {sample} channel {channel}"
                        ),
                    );
                    checks += 1;
                }
                expectations.push(json!({"linear": sample, "expected_rgb": expected}));
            }
            recorded.push(json!({
                "node": "color_wheels",
                "control": name,
                "position": position,
                "value": value,
                "expected": expectations,
            }));
        }
    }

    // §4.1 `bypass`: 0 evaluates the node, 1 is the exact identity.
    let bypass_descriptor = wheels_descriptor
        .parameter("bypass")
        .expect("bypass must be registered on color_wheels");
    assert_eq!(
        (
            bypass_descriptor.min,
            bypass_descriptor.max,
            bypass_descriptor.neutral
        ),
        (0, 1, 0)
    );
    let bypass_parameters = [("gain_red_thousandths", 1_200_i64)];
    for (token, identity) in [(0_i64, false), (1, true)] {
        let effect = with_parameter(&wheels_effect(1, &bypass_parameters), "bypass", token);
        let nodes = cpu_nodes(std::slice::from_ref(&effect));
        for sample in BOUNDARY_SAMPLES {
            let actual = apply_stack(&nodes, [sample; 3]);
            if identity {
                assert_eq!(
                    actual.map(f32::to_bits),
                    [sample.to_bits(); 3],
                    "bypass = 1 must be the exact identity at {sample}"
                );
            } else {
                let expected = spec_wheels_apply_f64(&bypass_parameters, [f64::from(sample); 3]);
                for channel in 0..3 {
                    assert_matches_spec(
                        actual[channel],
                        expected[channel],
                        &format!("color_wheels bypass=0 at {sample} channel {channel}"),
                    );
                    checks += 1;
                }
            }
        }
    }

    // §4.2 curve controls.
    for (name, bounds) in [
        ("master_point_count", (2_i64, 16_i64, 2_i64)),
        ("master_x0", (-2_000, 12_000, 0)),
        ("master_y0", (-2_000, 12_000, 0)),
        ("master_x15", (-2_000, 12_000, 10_000)),
        ("blue_y15", (-2_000, 12_000, 10_000)),
    ] {
        let parameter = curves_descriptor
            .parameter(name)
            .unwrap_or_else(|| panic!("{name} must be a registered color_curves parameter"));
        assert_eq!(
            (parameter.min, parameter.max, parameter.neutral),
            bounds,
            "{name} bounds drifted from the §4.2 table"
        );
    }

    for (name, points) in boundary_curve_cases() {
        let curves: [(ColorCurveChannel, &[(i64, i64)]); 1] =
            [(ColorCurveChannel::Master, points.as_slice())];
        let effect = curves_effect(2, &curves);
        let nodes = cpu_nodes(std::slice::from_ref(&effect));
        assert_eq!(nodes.len(), 1, "{name} is non-neutral and must stay active");
        let mut expectations = Vec::new();
        for sample in BOUNDARY_SAMPLES {
            let expected = spec_curves_apply_f64(&curves, [f64::from(sample); 3]);
            let actual = apply_stack(&nodes, [sample; 3]);
            for channel in 0..3 {
                assert_matches_spec(
                    actual[channel],
                    expected[channel],
                    &format!("color_curves {name} at linear {sample} channel {channel}"),
                );
                checks += 1;
            }
            expectations.push(json!({"linear": sample, "expected_rgb": expected}));
        }
        recorded.push(json!({
            "node": "color_curves",
            "control": name,
            "points": points,
            "point_count": points.len(),
            "expected": expectations,
        }));
    }

    // Per-channel curve composed with the master curve, so the §2.3 evaluation
    // order (channel first, then master) is itself covered by a written-out
    // expected value rather than only by the ordering fixture.
    let red_points = [(0, -500), (4_000, 5_500), (10_000, 10_000)];
    let master_points = [(0, 0), (5_000, 6_000), (10_000, 10_000)];
    let composed: [(ColorCurveChannel, &[(i64, i64)]); 2] = [
        (ColorCurveChannel::Red, &red_points),
        (ColorCurveChannel::Master, &master_points),
    ];
    let composed_effect = curves_effect(3, &composed);
    let composed_nodes = cpu_nodes(std::slice::from_ref(&composed_effect));
    for sample in BOUNDARY_SAMPLES {
        let expected = spec_curves_apply_f64(&composed, [f64::from(sample); 3]);
        let actual = apply_stack(&composed_nodes, [sample; 3]);
        for channel in 0..3 {
            assert_matches_spec(
                actual[channel],
                expected[channel],
                &format!("color_curves red+master at linear {sample} channel {channel}"),
            );
            checks += 1;
        }
        assert!(
            (f64::from(actual[0]) - expected[1]).abs() > 1.0e-6,
            "the red curve must not be applied to green as well at {sample}"
        );
    }

    let metrics = json!({
        "samples": BOUNDARY_SAMPLES,
        "expected_value_assertions": checks,
        "spec_relative_tolerance": SPEC_RELATIVE_TOLERANCE,
        "spec_absolute_floor": SPEC_ABSOLUTE_FLOOR,
        "cases": recorded,
    });
    emit_cc3_evidence(
        "cc3_boundary_expected_values",
        CPU_REFERENCE_BACKEND,
        CPU_REFERENCE_LANE,
        json!({"controls": WHEEL_CONTROLS.map(|control| control.0), "curve_cases": boundary_curve_cases().len()}),
        (0, 0),
        json_hash(&metrics),
        metrics,
    );
}

/// CC3 §10.3.4, second half: finiteness for each control at its bound
/// individually, the documented `slope = 16 ∧ power = 16` f32 overflow to
/// `+inf`, over-range survival, the wide diagonal curve, and `gain = 0`.
#[test]
fn cc3_boundary_controls_stay_finite_and_the_documented_extreme_overflows_to_infinity() {
    let raster = cc3_parity_raster();

    // §4.1: finite output for every raster sample when at most one of slope or
    // power sits at its maximum, i.e. for each control at its bound alone.
    let mut finite_checks = 0_usize;
    for (name, minimum, maximum, _, interior) in WHEEL_CONTROLS {
        for value in [minimum, maximum, interior] {
            let effect = wheels_effect(1, &[(name, value)]);
            let nodes = cpu_nodes(std::slice::from_ref(&effect));
            for sample in &raster {
                for channel in apply_stack(&nodes, *sample) {
                    assert!(
                        channel.is_finite(),
                        "color_wheels {name}={value} produced {channel} on {sample:?}"
                    );
                    finite_checks += 1;
                }
            }
        }
    }
    for (name, points) in boundary_curve_cases() {
        let curves: [(ColorCurveChannel, &[(i64, i64)]); 1] =
            [(ColorCurveChannel::Master, points.as_slice())];
        let effect = curves_effect(2, &curves);
        let nodes = cpu_nodes(std::slice::from_ref(&effect));
        for sample in &raster {
            for channel in apply_stack(&nodes, *sample) {
                assert!(
                    channel.is_finite(),
                    "color_curves {name} produced {channel} on {sample:?}"
                );
                finite_checks += 1;
            }
        }
    }

    // The documented simultaneous extreme. §4.1 states the linear-4.0 result is
    // mathematically ~1.1e53 and overflows f32 to +inf; it is asserted as
    // +inf, never excused, and never NaN.
    let extreme_parameters = [
        ("gain_master_thousandths", 4_000_i64),
        ("gain_red_thousandths", 4_000),
        ("gamma_master_thousandths", 4_000),
        ("gamma_red_thousandths", 4_000),
    ];
    let extreme = wheels_effect(3, &extreme_parameters);
    let extreme_nodes = cpu_nodes(std::slice::from_ref(&extreme));
    let extreme_expected = spec_wheels_apply_f64(&extreme_parameters, [4.0; 3]);
    assert!(
        extreme_expected[0].is_finite() && extreme_expected[0] > 1.0e50,
        "the f64 spec value for slope=16, power=16 on 4.0 should be ~1.1e53, not {}",
        extreme_expected[0]
    );
    let extreme_output = apply_stack(&extreme_nodes, [4.0; 3]);
    assert_eq!(
        extreme_output[0],
        f32::INFINITY,
        "slope = 16 and power = 16 on linear 4.0 must overflow f32 to +inf"
    );
    assert!(!extreme_output[0].is_nan());
    assert!(
        extreme_output[1].is_finite() && extreme_output[2].is_finite(),
        "only the red channel carries the simultaneous extreme: {extreme_output:?}"
    );
    let mut extreme_infinities = 0_usize;
    for sample in &raster {
        for channel in apply_stack(&extreme_nodes, *sample) {
            assert!(
                !channel.is_nan(),
                "the simultaneous extreme must never produce NaN on {sample:?}"
            );
            if channel.is_infinite() {
                extreme_infinities += 1;
            }
        }
    }
    assert!(
        extreme_infinities > 0,
        "the simultaneous extreme must actually reach the f32 overflow the contract documents"
    );

    // The final monitor clamp resolves the overflow, and no early clamp is
    // applied anywhere. On the CPU reference that means the +inf red channel
    // encodes to 255.
    //
    // The production GPU path cannot reproduce that code, and the fixture says
    // so rather than asserting an agreement that does not exist. The working
    // surface is `Rgba16Float`: `half::f16::from_f32` rounds an out-of-range
    // f32 to +/-inf per IEEE-754, while this adapter's f32->f16 image store
    // saturates to +/-65504 and turns a true f32 infinity into NaN, which the
    // monitor encode maps to 0. Both are legal resolutions of a documented
    // overflow and neither invents a mid-range colour, so the fixture asserts
    // that (a) the GPU working value is still at or beyond the half-float
    // limit, proving no early clamp, and (b) the monitor code sits at a clamp
    // extreme. The observed code is recorded in the evidence.
    //
    // This divergence is exactly why the simultaneous extreme is not part of
    // the §10.3.9 parity raster: there it would trip the `non_finite` counter,
    // which must stay zero for every case the gate does cover.
    let gpu = fallback_gpu();
    let compositor = Compositor::new(gpu.context());
    let (width, height, frame) = boundary_frame();
    let extreme_stack = [extreme.clone()];
    let cpu_monitor = cpu_reference_monitor(&frame, &extreme_nodes);
    let gpu_monitor_codes = gpu_monitor(&compositor, (width, height), &frame, &extreme_stack);
    let gpu_overflow_linear = gpu_linear(&compositor, (width, height), &frame, &extreme_stack);
    // The 4.0 block is the last of the four blocks in `boundary_frame`.
    let overflow_column = (width - CC3_RASTER_BLOCK_WIDTH / 2) as usize;
    assert_eq!(
        cpu_monitor[overflow_column * 4],
        255,
        "the CPU monitor encode must clamp the +inf red channel to 255"
    );
    let gpu_overflow_value = gpu_overflow_linear[overflow_column * 4];
    assert!(
        !gpu_overflow_value.is_finite() || gpu_overflow_value.abs() >= 65_504.0,
        "the production shader must not clamp the documented overflow early; it produced {gpu_overflow_value}"
    );
    let gpu_overflow_code = gpu_monitor_codes[overflow_column * 4];
    assert!(
        gpu_overflow_code == 0 || gpu_overflow_code == 255,
        "the production monitor encode must resolve the documented overflow at a clamp extreme, not at {gpu_overflow_code}"
    );
    assert!(
        gpu_monitor_codes
            .as_chunks::<4>()
            .0
            .iter()
            .all(|pixel| pixel[3] == 255),
        "alpha must survive the overflow case"
    );

    // Over-range input survives every stage: a mild node keeps 4.0 above 1.0.
    let mild = representative_wheels(4);
    let mild_nodes = cpu_nodes(std::slice::from_ref(&mild));
    let mild_output = apply_stack(&mild_nodes, [4.0; 3]);
    for channel in mild_output {
        assert!(
            channel > 1.0 && channel.is_finite(),
            "over-range input must survive the node: {mild_output:?}"
        );
    }

    // A curve whose points sit at -2000 and 12000 on the diagonal is identity
    // within LINEAR_CPU_GPU_MAX, and it is *not* structurally identity, so it
    // is really evaluated rather than short-circuited.
    let diagonal_points = [
        (COLOR_CURVE_COORDINATE_MIN, COLOR_CURVE_COORDINATE_MIN),
        (COLOR_CURVE_COORDINATE_MAX, COLOR_CURVE_COORDINATE_MAX),
    ];
    let diagonal = curves_effect(5, &[(ColorCurveChannel::Master, &diagonal_points)]);
    let resolved = ResolvedCurves::from_effect(&diagonal);
    assert!(
        !resolved.master.is_structural_identity(),
        "the wide diagonal is mathematically, not structurally, identity"
    );
    assert!(
        resolved.is_active(),
        "the wide diagonal node must be active"
    );
    let diagonal_nodes = cpu_nodes(std::slice::from_ref(&diagonal));
    assert_eq!(diagonal_nodes.len(), 1);
    let mut worst_diagonal = 0.0_f32;
    for sample in &raster {
        let output = apply_stack(&diagonal_nodes, *sample);
        for channel in 0..3 {
            let error = (output[channel] - sample[channel]).abs();
            assert!(
                error <= LINEAR_CPU_GPU_MAX,
                "the -2000..12000 diagonal curve moved {} to {} (error {error})",
                sample[channel],
                output[channel]
            );
            worst_diagonal = worst_diagonal.max(error);
        }
    }

    // `gain_* = 0` produces the documented constant, not an error.
    let zero_gain_parameters = [("gain_red_thousandths", 0_i64)];
    let zero_gain = wheels_effect(6, &zero_gain_parameters);
    let zero_gain_nodes = cpu_nodes(std::slice::from_ref(&zero_gain));
    for sample in &raster {
        let output = apply_stack(&zero_gain_nodes, *sample);
        assert_eq!(
            output[0].to_bits(),
            0.0_f32.to_bits(),
            "gain_red = 0 makes red the constant D(0) = 0, but produced {} on {sample:?}",
            output[0]
        );
    }
    // With a lift the constant is the documented non-zero value.
    let lifted_parameters = [
        ("gain_red_thousandths", 0_i64),
        ("lift_master_basis_points", 500),
    ];
    let lifted = wheels_effect(7, &lifted_parameters);
    let lifted_nodes = cpu_nodes(std::slice::from_ref(&lifted));
    let lifted_expected = spec_wheels_apply_f64(&lifted_parameters, [0.0; 3])[0];
    for sample in &raster {
        let output = apply_stack(&lifted_nodes, *sample);
        assert_matches_spec(
            output[0],
            lifted_expected,
            &format!("gain_red = 0 with lift_master = 500 on {sample:?}"),
        );
    }

    let metrics = json!({
        "lane": gpu.lane.id(),
        "finite_channel_assertions": finite_checks,
        "simultaneous_extreme": {
            "controls": extreme_parameters.map(|(name, value)| json!({"name": name, "value": value})),
            "slope": 16.0,
            "power": 16.0,
            "linear_input": 4.0,
            "f64_spec_value": extreme_expected[0],
            "f32_result": "inf",
            "nan_observed": false,
            "infinite_raster_channels": extreme_infinities,
            "cpu_monitor_code_after_clamp": 255,
            "gpu_working_linear": gpu_overflow_value.to_string(),
            "gpu_monitor_code_after_clamp": gpu_overflow_code,
            "gpu_half_float_note": "the Rgba16Float working surface saturates an out-of-range f32 on store and maps a true f32 infinity to NaN on this adapter; half::f16::from_f32 rounds to +/-inf instead",
        },
        "wide_diagonal_max_error": worst_diagonal,
        "wide_diagonal_gate": LINEAR_CPU_GPU_MAX,
        "zero_gain_constant": 0.0,
        "zero_gain_with_lift_constant": lifted_expected,
    });
    emit_cc3_evidence(
        "cc3_boundary_finiteness",
        gpu.backend(),
        gpu.lane.id(),
        json!({"raster_samples": raster.len()}),
        (width, height),
        json_hash(&metrics),
        metrics,
    );
}

// ---------------------------------------------------------------------------
// §10.3.5 / §10.3.6: per-channel independence and collinear identity.
// ---------------------------------------------------------------------------

/// CC3 §10.3.5. Changing only the red controls, or only the red curve, leaves
/// green and blue bit-identical on the CPU reference and on the GPU.
///
/// The comparison is between two *active* nodes that differ only in red. A
/// comparison against the node-removed stack would not be a per-channel test:
/// removing the node also removes green and blue's `E`/`D` round trip, so any
/// difference would be attributable to §3.3 rather than to channel coupling.
#[test]
fn cc3_red_only_changes_leave_green_and_blue_bit_identical() {
    let gpu = fallback_gpu();
    let compositor = Compositor::new(gpu.context());
    let (width, height, frame) = cc3_raster_frame();
    let resolution = (width, height);

    let red_curve_a = [(0, 0), (4_000, 3_000), (10_000, 10_000)];
    let red_curve_b = [(0, 800), (4_000, 5_600), (10_000, 9_400)];
    let shared_green = [(0, 0), (6_000, 5_200), (10_000, 10_000)];
    let cases: [(&str, Effect, Effect); 2] = [
        (
            "color_wheels",
            wheels_effect(
                1,
                &[
                    ("gain_red_thousandths", 1_200),
                    ("gain_green_thousandths", 1_100),
                ],
            ),
            wheels_effect(
                1,
                &[
                    ("gain_red_thousandths", 1_500),
                    ("lift_red_basis_points", 400),
                    ("gamma_red_thousandths", 800),
                    ("gain_green_thousandths", 1_100),
                ],
            ),
        ),
        (
            "color_curves",
            curves_effect(
                2,
                &[
                    (ColorCurveChannel::Red, &red_curve_a),
                    (ColorCurveChannel::Green, &shared_green),
                ],
            ),
            curves_effect(
                2,
                &[
                    (ColorCurveChannel::Red, &red_curve_b),
                    (ColorCurveChannel::Green, &shared_green),
                ],
            ),
        ),
    ];

    let mut recorded = Vec::new();
    for (name, baseline, variant) in cases {
        let baseline_stack = [baseline];
        let variant_stack = [variant];
        let baseline_cpu = cpu_reference_linear(&frame, &cpu_nodes(&baseline_stack));
        let variant_cpu = cpu_reference_linear(&frame, &cpu_nodes(&variant_stack));
        let baseline_gpu = gpu_linear(&compositor, resolution, &frame, &baseline_stack);
        let variant_gpu = gpu_linear(&compositor, resolution, &frame, &variant_stack);

        let mut changed_red = 0_usize;
        for (path, (baseline_values, variant_values)) in [
            ("cpu", (&baseline_cpu, &variant_cpu)),
            ("gpu", (&baseline_gpu, &variant_gpu)),
        ] {
            let mut red = 0_usize;
            for (baseline_pixel, variant_pixel) in baseline_values
                .as_chunks::<4>()
                .0
                .iter()
                .zip(variant_values.as_chunks::<4>().0.iter())
            {
                for channel in 1..4 {
                    assert_eq!(
                        variant_pixel[channel].to_bits(),
                        baseline_pixel[channel].to_bits(),
                        "{name} ({path}): a red-only change moved channel {channel}"
                    );
                }
                if variant_pixel[0].to_bits() != baseline_pixel[0].to_bits() {
                    red += 1;
                }
            }
            assert!(
                red * 10_000
                    >= (baseline_values.len() / 4) * MIN_CHANGED_LINEAR_BASIS_POINTS as usize,
                "{name} ({path}): the red-only change moved only {red} red samples, so the independence case proves nothing"
            );
            if path == "cpu" {
                changed_red = red;
            }
        }

        // The monitor path must agree too: an 8-bit encode that coupled
        // channels would be invisible in the linear comparison alone.
        let baseline_monitor = gpu_monitor(&compositor, resolution, &frame, &baseline_stack);
        let variant_monitor = gpu_monitor(&compositor, resolution, &frame, &variant_stack);
        for (baseline_pixel, variant_pixel) in baseline_monitor
            .as_chunks::<4>()
            .0
            .iter()
            .zip(variant_monitor.as_chunks::<4>().0.iter())
        {
            assert_eq!(
                &variant_pixel[1..],
                &baseline_pixel[1..],
                "{name}: a red-only change moved a non-red monitor code"
            );
        }
        recorded.push(json!({
            "node": name,
            "changed_red_samples": changed_red,
            "green_blue_alpha_bit_identical": true,
            "monitor_green_blue_identical": true,
        }));
    }

    let metrics = json!({"lane": gpu.lane.id(), "cases": recorded});
    emit_cc3_evidence(
        "cc3_per_channel_independence",
        gpu.backend(),
        gpu.lane.id(),
        json!({"changed": "red controls and red curve only"}),
        resolution,
        json_hash(&metrics),
        metrics,
    );
}

/// A 16-point curve with every point on the diagonal.
const SIXTEEN_POINT_DIAGONAL: [(i64, i64); 16] = [
    (0, 0),
    (667, 667),
    (1_333, 1_333),
    (2_000, 2_000),
    (2_667, 2_667),
    (3_333, 3_333),
    (4_000, 4_000),
    (4_667, 4_667),
    (5_333, 5_333),
    (6_000, 6_000),
    (6_667, 6_667),
    (7_333, 7_333),
    (8_000, 8_000),
    (8_667, 8_667),
    (9_333, 9_333),
    (10_000, 10_000),
];

/// CC3 §10.3.6. A 16-point collinear curve is identity within
/// `LINEAR_CPU_GPU_MAX` and is *not* short-circuited by §3.3.
#[test]
fn cc3_collinear_sixteen_point_curve_is_identity_without_the_short_circuit() {
    let gpu = fallback_gpu();
    let compositor = Compositor::new(gpu.context());
    let (width, height, frame) = cc3_raster_frame();

    let effect = curves_effect(
        1,
        &[
            (ColorCurveChannel::Master, &SIXTEEN_POINT_DIAGONAL),
            (ColorCurveChannel::Red, &SIXTEEN_POINT_DIAGONAL),
            (ColorCurveChannel::Green, &SIXTEEN_POINT_DIAGONAL),
            (ColorCurveChannel::Blue, &SIXTEEN_POINT_DIAGONAL),
        ],
    );
    let resolved = ResolvedCurves::from_effect(&effect);
    assert!(
        resolved.is_active(),
        "a collinear curve is mathematically identity but must still be evaluated"
    );
    assert_eq!(color_node_inactive_reason(&effect), None);
    for curve in ColorCurveChannel::ALL {
        assert!(
            !resolved.curve(curve).is_structural_identity(),
            "{curve:?} took the §3.3 structural short-circuit"
        );
        assert_eq!(resolved.curve(curve).points.len(), 16);
    }
    let stack = [effect];
    let nodes = cpu_nodes(&stack);
    assert_eq!(nodes.len(), 1);
    assert_eq!(nodes[0].kind(), ColorNodeKind::Curves);
    assert_ne!(
        crate::compositor::grade_buffer_bytes(&stack).expect("collinear stack serializes"),
        crate::compositor::grade_buffer_bytes(&[]).expect("empty stack serializes"),
        "an active collinear curve must reach the GPU buffer"
    );

    let mut worst_cpu = 0.0_f32;
    for sample in cc3_parity_raster() {
        let output = apply_stack(&nodes, sample);
        for channel in 0..3 {
            let error = (output[channel] - sample[channel]).abs();
            assert!(
                error <= LINEAR_CPU_GPU_MAX,
                "collinear curve moved {} to {} (error {error})",
                sample[channel],
                output[channel]
            );
            worst_cpu = worst_cpu.max(error);
        }
    }

    let rendered = gpu_linear(&compositor, (width, height), &frame, &stack);
    let source = frame
        .pixels
        .iter()
        .map(|value| value.to_f32())
        .collect::<Vec<_>>();
    let mut worst_gpu = 0.0_f32;
    for (rendered_pixel, source_pixel) in rendered
        .as_chunks::<4>()
        .0
        .iter()
        .zip(source.as_chunks::<4>().0.iter())
    {
        for channel in 0..3 {
            let error = (rendered_pixel[channel] - source_pixel[channel]).abs();
            assert!(
                error <= LINEAR_CPU_GPU_MAX,
                "production shader moved {} to {} through a collinear curve",
                source_pixel[channel],
                rendered_pixel[channel]
            );
            worst_gpu = worst_gpu.max(error);
        }
    }

    let metrics = json!({
        "lane": gpu.lane.id(),
        "points": SIXTEEN_POINT_DIAGONAL,
        "short_circuited": false,
        "cpu_max_identity_error": worst_cpu,
        "gpu_max_identity_error": worst_gpu,
        "gate": LINEAR_CPU_GPU_MAX,
    });
    emit_cc3_evidence(
        "cc3_collinear_identity",
        gpu.backend(),
        gpu.lane.id(),
        json!({"curves": "sixteen collinear points on all four channels"}),
        (width, height),
        json_hash(&metrics),
        metrics,
    );
}

// ---------------------------------------------------------------------------
// §10.3.7 / §10.3.8: ordering and degenerate automation.
// ---------------------------------------------------------------------------

fn cc3_asset() -> kinewright_core::MediaAsset {
    kinewright_core::MediaAsset {
        id: AssetId(1),
        path: std::path::PathBuf::from("cc3-fixture.mp4"),
        name: "cc3 fixture".to_owned(),
        duration: TimeCode(30),
        fps: kinewright_core::Rational::new(30, 1).expect("cc3 fixture fps"),
        kind: kinewright_core::MediaKind::Video,
        resolution: Some((16, 16)),
        source_fingerprint: kinewright_core::MediaSourceFingerprint::default(),
        color_description: kinewright_core::ColorDescription::default(),
    }
}

fn cc3_document() -> Document {
    simple_document(cc3_asset(), (16, 16))
}

fn clip_effects(document: &Document) -> &[Effect] {
    &document.tracks[0].clips[0].effects
}

/// CC3 §10.3.7. `[wheels, curves]` and `[curves, wheels]` produce different
/// results, each matches the CPU reference evaluated in the same vector order,
/// and a three-node `[primary, wheels, curves]` stack is checked as well.
#[test]
fn cc3_node_order_changes_the_result_and_each_order_matches_the_cpu_reference() {
    let gpu = fallback_gpu();
    let compositor = Compositor::new(gpu.context());
    let (width, height, frame) = cc3_raster_frame();
    let resolution = (width, height);
    let baseline = cpu_reference_linear(&frame, &[]);

    let wheels = representative_wheels(1);
    let curves = representative_curves(2);
    let primary = primary_effect(3);
    let stacks: Vec<(&str, Vec<Effect>, Vec<ColorNodeKind>)> = vec![
        (
            "wheels_then_curves",
            vec![wheels.clone(), curves.clone()],
            vec![ColorNodeKind::Wheels, ColorNodeKind::Curves],
        ),
        (
            "curves_then_wheels",
            vec![curves.clone(), wheels.clone()],
            vec![ColorNodeKind::Curves, ColorNodeKind::Wheels],
        ),
        (
            "primary_wheels_curves",
            vec![primary, wheels, curves],
            vec![
                ColorNodeKind::Primary,
                ColorNodeKind::Wheels,
                ColorNodeKind::Curves,
            ],
        ),
    ];

    let mut linear_by_stack: BTreeMap<&str, Vec<f32>> = BTreeMap::new();
    let mut monitor_by_stack: BTreeMap<&str, Vec<u8>> = BTreeMap::new();
    let mut recorded = Vec::new();
    for (name, stack, kinds) in &stacks {
        let nodes = cpu_nodes(stack);
        assert_eq!(
            nodes.iter().map(ColorNode::kind).collect::<Vec<_>>(),
            *kinds,
            "{name}: the resolved stack must follow clip.effects vector order"
        );
        assert_eq!(
            active_color_nodes(stack),
            kinds
                .iter()
                .copied()
                .enumerate()
                .collect::<Vec<(usize, ColorNodeKind)>>(),
            "{name}: stage indices must be the serialized positions"
        );
        let (monitor, linear, monitor_bytes) = assert_gpu_case(
            &compositor,
            resolution,
            &frame,
            stack,
            Some(&baseline),
            name,
        );
        linear_by_stack.insert(name, cpu_reference_linear(&frame, &nodes));
        monitor_by_stack.insert(name, monitor_bytes);
        recorded.push(json!({
            "stack": name,
            "kinds": kinds.iter().map(|kind| kind.effect_name()).collect::<Vec<_>>(),
            "monitor_max_code_error": monitor.max,
            "monitor_p99_code_error": monitor.p99,
            "monitor_mean_code_error": monitor.mean,
            "linear": linear.as_json(),
        }));
    }

    let forward = &linear_by_stack["wheels_then_curves"];
    let reverse = &linear_by_stack["curves_then_wheels"];
    let mut worst = 0.0_f32;
    let mut differing = 0_usize;
    for (a, b) in forward
        .as_chunks::<4>()
        .0
        .iter()
        .zip(reverse.as_chunks::<4>().0.iter())
    {
        for channel in 0..3 {
            let difference = (a[channel] - b[channel]).abs();
            if difference > 0.0 {
                differing += 1;
            }
            worst = worst.max(difference);
        }
    }
    assert!(
        worst > LINEAR_CPU_GPU_MAX,
        "the two node orders must differ by more than the parity gate, or the ordering claim is vacuous: max difference {worst}"
    );
    assert!(
        differing * 10_000 >= (forward.len() / 4 * 3) * MIN_CHANGED_LINEAR_BASIS_POINTS as usize,
        "only {differing} samples differ between the two orders"
    );
    assert_ne!(
        monitor_by_stack["wheels_then_curves"], monitor_by_stack["curves_then_wheels"],
        "the two orders must also differ after monitor encoding"
    );

    let metrics = json!({
        "lane": gpu.lane.id(),
        "order_max_linear_difference": worst,
        "order_differing_rgb_samples": differing,
        "stacks": recorded,
        "linear_gate": {
            "in_gamut": {"max": LINEAR_CPU_GPU_MAX, "p99": LINEAR_CPU_GPU_P99, "mean": LINEAR_CPU_GPU_MEAN},
            "over_range": {"max": LINEAR_CPU_GPU_MAX, "p99": LINEAR_OVER_RANGE_P99, "mean": LINEAR_OVER_RANGE_MEAN},
        },
        "monitor_gate": {"max": MONITOR_CPU_GPU_MAX, "p99": MONITOR_CPU_GPU_P99, "mean": MONITOR_CPU_GPU_MEAN},
    });
    emit_cc3_evidence(
        "cc3_node_ordering",
        gpu.backend(),
        gpu.lane.id(),
        json!({"orders": ["wheels_then_curves", "curves_then_wheels", "primary_wheels_curves"]}),
        resolution,
        json_hash(&metrics),
        metrics,
    );
}

/// CC3 §10.3.8. A keyframed `point_count` plus coordinates that cross drives
/// the §3.4 truncation rule; the CPU reference and the production shader agree
/// on the truncated curve.
#[test]
fn cc3_degenerate_automation_truncates_on_cpu_and_gpu() {
    const CROSSING_FRAME: i64 = 20;

    // Built through the ordinary edit path, so the document is provably legal:
    // §6 policy 2 allows keyframed coordinates while `point_count` carries at
    // most one keyframe, and that keyframe must be `Hold`.
    let mut document = cc3_document();
    let base = curves_effect(
        11,
        &[(
            ColorCurveChannel::Master,
            &[(0, 0), (3_000, 3_500), (6_000, 6_500), (10_000, 10_000)],
        )],
    );
    Operation::AddEffect {
        clip: ClipId(1),
        effect: base,
    }
    .apply(&mut document)
    .expect("a legal color_curves node must be accepted");
    Operation::SetEffectKeyframes {
        clip: ClipId(1),
        effect: EffectId(11),
        name: "master_point_count".to_owned(),
        curve: AutomationCurve {
            keyframes: vec![Keyframe {
                at: TimeCode(0),
                value: 4,
                interpolation: KeyframeInterpolation::Hold,
            }],
        },
    }
    .apply(&mut document)
    .expect("a single Hold keyframe on point_count is legal");
    Operation::SetEffectKeyframes {
        clip: ClipId(1),
        effect: EffectId(11),
        name: "master_x1".to_owned(),
        curve: AutomationCurve {
            keyframes: vec![
                Keyframe {
                    at: TimeCode(0),
                    value: 3_000,
                    interpolation: KeyframeInterpolation::Linear,
                },
                Keyframe {
                    at: TimeCode(CROSSING_FRAME),
                    value: 7_000,
                    interpolation: KeyframeInterpolation::Linear,
                },
            ],
        },
    }
    .apply(&mut document)
    .expect("coordinates may be keyframed while point_count has one keyframe");

    let stored = clip_effects(&document)[0].clone();
    assert_eq!(stored.name, "color_curves");

    let settled = stored.evaluated_at(TimeCode(0));
    let settled_curves = ResolvedCurves::from_effect(&settled);
    assert!(
        !settled_curves.truncated(),
        "the curve is well ordered before the coordinates cross"
    );
    assert_eq!(settled_curves.master.points.len(), 4);

    let crossed = stored.evaluated_at(TimeCode(CROSSING_FRAME));
    let crossed_curves = ResolvedCurves::from_effect(&crossed);
    assert!(
        crossed_curves.truncated(),
        "crossing coordinates must set the §3.4 truncation flag"
    );
    assert_eq!(
        crossed_curves.truncated_curves(),
        vec![ColorCurveChannel::Master]
    );
    assert_eq!(crossed_curves.master.declared_point_count, 4);
    assert_eq!(
        crossed_curves.master.points,
        vec![(0, 0), (7_000, 3_500)],
        "§3.4 keeps the longest strictly-increasing-x prefix, with no reordering and no clamping"
    );
    assert!(crossed_curves.is_active());

    let gpu = fallback_gpu();
    let compositor = Compositor::new(gpu.context());
    let (width, height, frame) = cc3_raster_frame();
    let resolution = (width, height);
    let baseline = cpu_reference_linear(&frame, &[]);
    let stack = [crossed.clone()];
    let (monitor, linear, _) = assert_gpu_case(
        &compositor,
        resolution,
        &frame,
        &stack,
        Some(&baseline),
        "degenerate_automation_truncated",
    );

    // The truncated curve must not be the same node as the untruncated one, or
    // "both paths agree" would be trivially true.
    let settled_linear = cpu_reference_linear(&frame, &cpu_nodes(&[settled]));
    let crossed_linear = cpu_reference_linear(&frame, &cpu_nodes(&stack));
    assert_ne!(
        bits_of(&settled_linear),
        bits_of(&crossed_linear),
        "truncation must actually change the rendered result"
    );

    let metrics = json!({
        "lane": gpu.lane.id(),
        "crossing_frame": CROSSING_FRAME,
        "declared_point_count": crossed_curves.master.declared_point_count,
        "resolved_points": crossed_curves.master.points,
        "truncated": true,
        "truncated_curves": ["master"],
        "monitor_max_code_error": monitor.max,
        "monitor_p99_code_error": monitor.p99,
        "monitor_mean_code_error": monitor.mean,
        "linear": linear.as_json(),
    });
    emit_cc3_evidence(
        "cc3_degenerate_automation",
        gpu.backend(),
        gpu.lane.id(),
        json!({"policy": "point_count Hold with one keyframe, coordinates Linear"}),
        resolution,
        json_hash(&metrics),
        metrics,
    );
}

// ---------------------------------------------------------------------------
// §10.3.9: CPU/GPU parity.
// ---------------------------------------------------------------------------

fn assert_cc3_gpu_parity(gpu: &FixtureGpu) {
    let backend = gpu.backend().to_owned();
    let compositor = Compositor::new(gpu.context());
    let (width, height, frame) = cc3_raster_frame();
    let resolution = (width, height);
    let baseline = cpu_reference_linear(&frame, &[]);
    let mut cases = Vec::new();
    let mut output_bytes = Vec::new();
    let mut above_domain_total = 0_usize;
    let mut non_finite_total = 0_usize;

    // The §6.2 neutral-identity numbers, reused verbatim: an empty node stack
    // must reproduce the CPU reference within one monitor code.
    let neutral_expected = cpu_reference_monitor(&frame, &[]);
    let neutral_actual = gpu_monitor(&compositor, resolution, &frame, &[]);
    let neutral_metric = abs_code_diff_rgb(&neutral_actual, &neutral_expected);
    assert!(
        neutral_metric.max <= IDENTITY_RAMP_MONITOR_MAX,
        "neutral identity monitor max: {neutral_metric:?}"
    );
    assert!(
        neutral_metric.p99 <= IDENTITY_RAMP_MONITOR_P99,
        "neutral identity monitor P99: {neutral_metric:?}"
    );
    assert!(
        neutral_metric.mean <= IDENTITY_RAMP_MONITOR_MEAN,
        "neutral identity monitor mean: {neutral_metric:?}"
    );

    let full_stack = vec![
        primary_effect(1),
        representative_wheels(2),
        representative_curves(3),
    ];
    let (monitor, linear, monitor_bytes) = assert_gpu_case(
        &compositor,
        resolution,
        &frame,
        &full_stack,
        Some(&baseline),
        "representative_full_stack",
    );
    let full_stack_luma = monitor_luma_and_clipping(&monitor_bytes);
    let full_stack_metrics = json!({
        "case": "representative_full_stack",
        "nodes": ["primary_correction", "color_wheels", "color_curves"],
        "monitor_max_code_error": monitor.max,
        "monitor_p99_code_error": monitor.p99,
        "monitor_mean_code_error": monitor.mean,
        "linear": linear.as_json(),
        "monitor_luma_and_clipping": full_stack_luma.clone(),
    });
    above_domain_total += linear.above_domain;
    non_finite_total += linear.non_finite;
    output_bytes.extend_from_slice(&monitor_bytes);
    cases.push(full_stack_metrics.clone());

    for (name, minimum, maximum, _, interior) in WHEEL_CONTROLS {
        for (position, value) in [
            ("minimum", minimum),
            ("maximum", maximum),
            ("interior", interior),
        ] {
            let label = format!("color_wheels {name}={value}");
            let stack = [wheels_effect(10, &[(name, value)])];
            let (monitor, linear, bytes) = assert_gpu_case(
                &compositor,
                resolution,
                &frame,
                &stack,
                Some(&baseline),
                &label,
            );
            above_domain_total += linear.above_domain;
            non_finite_total += linear.non_finite;
            output_bytes.extend_from_slice(&bytes);
            cases.push(json!({
                "case": "wheels_control_boundary",
                "control": name,
                "position": position,
                "value": value,
                "monitor_max_code_error": monitor.max,
                "monitor_p99_code_error": monitor.p99,
                "monitor_mean_code_error": monitor.mean,
                "linear": linear.as_json(),
            }));
        }
    }

    for (name, points) in boundary_curve_cases() {
        let label = format!("color_curves {name}");
        let stack = [curves_effect(
            11,
            &[(ColorCurveChannel::Master, points.as_slice())],
        )];
        let (monitor, linear, bytes) = assert_gpu_case(
            &compositor,
            resolution,
            &frame,
            &stack,
            Some(&baseline),
            &label,
        );
        above_domain_total += linear.above_domain;
        non_finite_total += linear.non_finite;
        output_bytes.extend_from_slice(&bytes);
        cases.push(json!({
            "case": "curve_boundary",
            "control": name,
            "points": points,
            "monitor_max_code_error": monitor.max,
            "monitor_p99_code_error": monitor.p99,
            "monitor_mean_code_error": monitor.mean,
            "linear": linear.as_json(),
        }));
    }

    assert_eq!(
        non_finite_total, 0,
        "a non-finite sample must never be excluded from the CC3 linear gate"
    );

    emit_cc3_evidence(
        "cc3_gpu_cpu_parity",
        &backend,
        gpu.lane.id(),
        json!({
            "raster": "cc3_parity_raster",
            "cases": "representative_full_stack_plus_every_control_boundary",
        }),
        resolution,
        output_hash(&output_bytes),
        json!({
            "lane": gpu.lane.id(),
            "linear_storage": "rgba16float",
            "case_count": cases.len(),
            "representative_full_stack": full_stack_metrics,
            "neutral_identity_monitor": {
                "max_code_error": neutral_metric.max,
                "p99_code_error": neutral_metric.p99,
                "mean_code_error": neutral_metric.mean,
                "gate": {
                    "max": IDENTITY_RAMP_MONITOR_MAX,
                    "p99": IDENTITY_RAMP_MONITOR_P99,
                    "mean": IDENTITY_RAMP_MONITOR_MEAN,
                },
            },
            "above_domain_rgb_samples_total": above_domain_total,
            "non_finite_rgb_samples_total": non_finite_total,
            "cases": cases,
            "monitor_gate": {"max": MONITOR_CPU_GPU_MAX, "p99": MONITOR_CPU_GPU_P99, "mean": MONITOR_CPU_GPU_MEAN},
            "linear_gate": {
                "in_gamut": {"max": LINEAR_CPU_GPU_MAX, "p99": LINEAR_CPU_GPU_P99, "mean": LINEAR_CPU_GPU_MEAN},
                "over_range": {"max": LINEAR_CPU_GPU_MAX, "p99": LINEAR_OVER_RANGE_P99, "mean": LINEAR_OVER_RANGE_MEAN},
                "above_domain_excluded_above": LINEAR_GATE_DOMAIN,
                "non_finite_allowed": 0,
                "status": "passed",
            },
        }),
    );
}

/// CC3 §10.3.9 on the default lane.
#[test]
fn cc3_gpu_compositor_matches_the_cpu_reference_on_software_fallback() {
    assert_cc3_gpu_parity(&fallback_gpu());
}

/// CC3 §10.3.9 on the explicit hardware lane.
#[test]
#[ignore = "requires a physical supported GPU; run explicitly with --ignored --nocapture"]
fn cc3_gpu_compositor_matches_the_cpu_reference_on_hardware() {
    assert_cc3_gpu_parity(&hardware_gpu());
}

// ---------------------------------------------------------------------------
// §10.3.10: serialization, history, and typed rejections.
// ---------------------------------------------------------------------------

/// A 16-point curve with strictly increasing `x` and the given `y` offset.
fn sixteen_point_curve(offset: i64) -> Vec<(i64, i64)> {
    SIXTEEN_POINT_MONOTONE
        .into_iter()
        .map(|(x, y)| {
            (
                x,
                (y + offset).clamp(COLOR_CURVE_COORDINATE_MIN, COLOR_CURVE_COORDINATE_MAX),
            )
        })
        .collect()
}

fn document_from(event: Event, label: &str) -> Arc<Document> {
    match event {
        Event::DocumentChanged { doc, .. } => doc,
        other => panic!("{label} was not an accepted document state: {other:?}"),
    }
}

/// CC3 §10.3.10. Save/reopen, journal replay, undo, and redo preserve both
/// nodes, their values, and their vector position exactly.
#[test]
fn cc3_serialization_and_history_preserve_both_nodes() {
    let master = sixteen_point_curve(0);
    let red = sixteen_point_curve(200);
    let green = sixteen_point_curve(-150);
    let blue = sixteen_point_curve(350);
    let curves = curves_effect(
        22,
        &[
            (ColorCurveChannel::Master, &master),
            (ColorCurveChannel::Red, &red),
            (ColorCurveChannel::Green, &green),
            (ColorCurveChannel::Blue, &blue),
        ],
    );
    assert_eq!(
        curves.parameters.len(),
        4 * (1 + 2 * 16),
        "a 16-point curve on all four channels stores 132 integers"
    );
    let wheels = representative_wheels(21);

    let core = Core::spawn(cc3_document()).expect("cc3 history core");
    let added = document_from(
        core.request(Command::DoBatch(vec![
            Operation::AddEffect {
                clip: ClipId(1),
                effect: wheels.clone(),
            },
            Operation::AddEffect {
                clip: ClipId(1),
                effect: curves.clone(),
            },
        ]))
        .expect("both CC3 nodes must be accepted"),
        "AddEffect batch",
    );
    assert_eq!(
        clip_effects(&added)
            .iter()
            .map(|effect| effect.name.as_str())
            .collect::<Vec<_>>(),
        vec!["color_wheels", "color_curves"]
    );
    assert_eq!(clip_effects(&added)[0], wheels);
    assert_eq!(clip_effects(&added)[1], curves);

    // Save and reopen.
    let saved = serde_json::to_vec(added.as_ref()).expect("CC3 document must serialize");
    let reopened: Document = serde_json::from_slice(&saved).expect("CC3 document must reopen");
    assert_eq!(&reopened, added.as_ref());
    assert_eq!(clip_effects(&reopened)[1].parameters, curves.parameters);

    // Journal replay through the public actor boundary.
    let replay_core = Core::spawn(cc3_document()).expect("cc3 replay core");
    let journal = JournalCommand::DoBatch(vec![
        Operation::AddEffect {
            clip: ClipId(1),
            effect: wheels.clone(),
        },
        Operation::AddEffect {
            clip: ClipId(1),
            effect: curves.clone(),
        },
    ]);
    let wire = serde_json::to_value(&journal).expect("journal command must serialize");
    let decoded: JournalCommand =
        serde_json::from_value(wire).expect("journal command must deserialize");
    let replayed = document_from(
        replay_core
            .request(decoded.into())
            .expect("journal replay must apply"),
        "journal replay",
    );
    assert_eq!(clip_effects(&replayed), clip_effects(&added));

    // A second batch: SetEffectParam, SetEffectKeyframes, ClearEffectKeyframes.
    let edited = document_from(
        core.request(Command::DoBatch(vec![
            Operation::SetEffectParam {
                clip: ClipId(1),
                effect: EffectId(21),
                name: "gain_green_thousandths".to_owned(),
                value: ParamValue::Integer(1_300),
            },
            Operation::SetEffectKeyframes {
                clip: ClipId(1),
                effect: EffectId(21),
                name: "bypass".to_owned(),
                curve: AutomationCurve {
                    keyframes: vec![
                        Keyframe {
                            at: TimeCode(0),
                            value: 0,
                            interpolation: KeyframeInterpolation::Hold,
                        },
                        Keyframe {
                            at: TimeCode(15),
                            value: 1,
                            interpolation: KeyframeInterpolation::Hold,
                        },
                    ],
                },
            },
            Operation::SetEffectKeyframes {
                clip: ClipId(1),
                effect: EffectId(22),
                name: "master_y3".to_owned(),
                curve: AutomationCurve {
                    keyframes: vec![Keyframe {
                        at: TimeCode(5),
                        value: 2_100,
                        interpolation: KeyframeInterpolation::Linear,
                    }],
                },
            },
        ]))
        .expect("CC3 parameter and keyframe edits must be accepted"),
        "edit batch",
    );
    assert_eq!(
        clip_effects(&edited)[0].parameters["gain_green_thousandths"],
        ParamValue::Integer(1_300)
    );
    assert!(clip_effects(&edited)[0].keyframes.contains_key("bypass"));
    assert!(clip_effects(&edited)[1].keyframes.contains_key("master_y3"));
    // The bypass automation resolves through the `>= 1` test.
    assert_eq!(
        color_node_inactive_reason(&clip_effects(&edited)[0].evaluated_at(TimeCode(0))),
        None
    );
    assert_eq!(
        color_node_inactive_reason(&clip_effects(&edited)[0].evaluated_at(TimeCode(20))),
        Some(ColorNodeInactiveReason::Bypassed)
    );

    let cleared = document_from(
        core.request(Command::Do(Operation::ClearEffectKeyframes {
            clip: ClipId(1),
            effect: EffectId(22),
            name: "master_y3".to_owned(),
        }))
        .expect("ClearEffectKeyframes must be accepted"),
        "clear keyframes",
    );
    assert!(
        !clip_effects(&cleared)[1]
            .keyframes
            .contains_key("master_y3")
    );

    // Undo unwinds one history entry at a time and redo restores it exactly.
    let undone_clear = document_from(
        core.request(Command::Undo).expect("undo clear"),
        "undo clear",
    );
    assert_eq!(clip_effects(&undone_clear), clip_effects(&edited));
    let undone_edit = document_from(core.request(Command::Undo).expect("undo edit"), "undo edit");
    assert_eq!(clip_effects(&undone_edit), clip_effects(&added));
    let undone_add = document_from(core.request(Command::Undo).expect("undo add"), "undo add");
    assert!(
        clip_effects(&undone_add).is_empty(),
        "undoing the add batch must remove both nodes atomically"
    );
    let redone_add = document_from(core.request(Command::Redo).expect("redo add"), "redo add");
    assert_eq!(clip_effects(&redone_add), clip_effects(&added));
    let redone_edit = document_from(core.request(Command::Redo).expect("redo edit"), "redo edit");
    assert_eq!(clip_effects(&redone_edit), clip_effects(&edited));
    let redone_clear = document_from(
        core.request(Command::Redo).expect("redo clear"),
        "redo clear",
    );
    assert_eq!(clip_effects(&redone_clear), clip_effects(&cleared));

    let metrics = json!({
        "curve_parameters_stored": curves.parameters.len(),
        "vector_order": ["color_wheels", "color_curves"],
        "save_reopen": true,
        "journal_replay": true,
        "undo_steps": 3,
        "redo_steps": 3,
        "operations": ["AddEffect", "SetEffectParam", "SetEffectKeyframes", "ClearEffectKeyframes"],
    });
    emit_cc3_evidence(
        "cc3_serialization_history",
        "backend=kinewright_core_actor;adapter=host;software_fallback=true;gpu_claim=false;lane=cpu_reference",
        CPU_REFERENCE_LANE,
        json!({"nodes": ["color_wheels", "color_curves"], "curve_points": 16}),
        (0, 0),
        json_hash(&metrics),
        metrics,
    );
}

/// CC3 §10.3.10 and fixture-quality rule §10.1.6. Every rejection is typed and
/// carries the field, the observed value, and the allowed values.
#[test]
fn cc3_illegal_edits_are_rejected_atomically_with_field_observed_and_allowed() {
    let mut document = cc3_document();
    Operation::AddEffect {
        clip: ClipId(1),
        effect: representative_wheels(31),
    }
    .apply(&mut document)
    .expect("a legal wheels node");
    Operation::AddEffect {
        clip: ClipId(1),
        effect: curves_effect(
            32,
            &[(
                ColorCurveChannel::Master,
                &[(0, 0), (5_000, 6_000), (10_000, 10_000)],
            )],
        ),
    }
    .apply(&mut document)
    .expect("a legal curves node");
    let untouched = document.clone();
    let mut recorded = Vec::new();

    let mut expect_rejection = |operation: Operation, label: &str| -> OpError {
        let mut candidate = untouched.clone();
        let error = operation
            .apply(&mut candidate)
            .expect_err(&format!("{label} must be rejected"));
        assert_eq!(
            candidate, untouched,
            "{label} must be rejected atomically, leaving the document byte-identical"
        );
        recorded.push(json!({"case": label, "error": error.to_string()}));
        error
    };

    // Out of range: field, observed, allowed.
    let out_of_range = expect_rejection(
        Operation::SetEffectParam {
            clip: ClipId(1),
            effect: EffectId(31),
            name: "gain_master_thousandths".to_owned(),
            value: ParamValue::Integer(4_001),
        },
        "gain_master_thousandths = 4001",
    );
    match &out_of_range {
        OpError::EffectParamOutOfRange {
            effect,
            name,
            min,
            max,
            actual,
        } => {
            assert_eq!(effect, "color_wheels");
            assert_eq!(name, "gain_master_thousandths");
            assert_eq!((*min, *max, *actual), (0, 4_000, 4_001));
        }
        other => panic!("expected EffectParamOutOfRange, got {other:?}"),
    }
    assert_eq!(
        out_of_range.to_string(),
        "effect \"color_wheels\" parameter \"gain_master_thousandths\" is 4001, outside the inclusive range 0..=4000"
    );

    // Wrong type.
    let wrong_type = expect_rejection(
        Operation::SetEffectParam {
            clip: ClipId(1),
            effect: EffectId(32),
            name: "master_x1".to_owned(),
            value: ParamValue::Text("5000".to_owned()),
        },
        "master_x1 = text",
    );
    match &wrong_type {
        OpError::InvalidEffectParamType { effect, name } => {
            assert_eq!(effect, "color_curves");
            assert_eq!(name, "master_x1");
        }
        other => panic!("expected InvalidEffectParamType, got {other:?}"),
    }
    assert_eq!(
        wrong_type.to_string(),
        "effect \"color_curves\" parameter \"master_x1\" requires an integer"
    );

    // Unknown parameter.
    let unknown = expect_rejection(
        Operation::SetEffectParam {
            clip: ClipId(1),
            effect: EffectId(32),
            name: "luma_point_count".to_owned(),
            value: ParamValue::Integer(4),
        },
        "luma_point_count",
    );
    match &unknown {
        OpError::UnknownEffectParam { effect, name } => {
            assert_eq!(effect, "color_curves");
            assert_eq!(name, "luma_point_count");
        }
        other => panic!("expected UnknownEffectParam, got {other:?}"),
    }
    assert_eq!(
        unknown.to_string(),
        "effect \"color_curves\" has no parameter \"luma_point_count\""
    );

    // Non-increasing x over the active prefix.
    let non_increasing = expect_rejection(
        Operation::AddEffect {
            clip: ClipId(1),
            effect: curves_effect(
                33,
                &[(
                    ColorCurveChannel::Green,
                    &[(0, 0), (5_000, 4_000), (5_000, 6_000)],
                )],
            ),
        },
        "green x2 == x1",
    );
    match &non_increasing {
        OpError::InvalidCurvePoints {
            effect,
            curve,
            index,
            previous_x,
            x,
        } => {
            assert_eq!(effect, "color_curves");
            assert_eq!(curve, "green");
            assert_eq!((*index, *previous_x, *x), (2, 5_000, 5_000));
        }
        other => panic!("expected InvalidCurvePoints, got {other:?}"),
    }
    assert!(
        non_increasing
            .to_string()
            .contains("x must be strictly increasing over the active prefix"),
        "the rejection must state the rule: {non_increasing}"
    );

    // point_count accepts only Hold keyframes.
    let non_hold = expect_rejection(
        Operation::SetEffectKeyframes {
            clip: ClipId(1),
            effect: EffectId(32),
            name: "master_point_count".to_owned(),
            curve: AutomationCurve {
                keyframes: vec![
                    Keyframe {
                        at: TimeCode(0),
                        value: 3,
                        interpolation: KeyframeInterpolation::Linear,
                    },
                    Keyframe {
                        at: TimeCode(10),
                        value: 5,
                        interpolation: KeyframeInterpolation::Linear,
                    },
                ],
            },
        },
        "master_point_count with Linear keyframes",
    );
    match &non_hold {
        OpError::NonHoldKeyframeParameter { effect, name } => {
            assert_eq!(effect, "color_curves");
            assert_eq!(name, "master_point_count");
        }
        other => panic!("expected NonHoldKeyframeParameter, got {other:?}"),
    }
    assert_eq!(
        non_hold.to_string(),
        "effect \"color_curves\" parameter \"master_point_count\" accepts only hold keyframes"
    );

    // An animated point_count and animated coordinates are exclusive.
    let mut animated_point_count = untouched.clone();
    Operation::SetEffectKeyframes {
        clip: ClipId(1),
        effect: EffectId(32),
        name: "master_point_count".to_owned(),
        curve: AutomationCurve {
            keyframes: vec![
                Keyframe {
                    at: TimeCode(0),
                    value: 3,
                    interpolation: KeyframeInterpolation::Hold,
                },
                Keyframe {
                    at: TimeCode(10),
                    value: 2,
                    interpolation: KeyframeInterpolation::Hold,
                },
            ],
        },
    }
    .apply(&mut animated_point_count)
    .expect("two Hold keyframes on point_count are policy 1");
    let baseline = animated_point_count.clone();
    let mixed = Operation::SetEffectKeyframes {
        clip: ClipId(1),
        effect: EffectId(32),
        name: "master_x1".to_owned(),
        curve: AutomationCurve {
            keyframes: vec![Keyframe {
                at: TimeCode(0),
                value: 4_000,
                interpolation: KeyframeInterpolation::Linear,
            }],
        },
    }
    .apply(&mut animated_point_count)
    .expect_err("policy 1 and policy 2 must not be mixed");
    assert_eq!(animated_point_count, baseline);
    match &mixed {
        OpError::CurvePointCountAnimatedWithPoints { effect, curve } => {
            assert_eq!(effect, "color_curves");
            assert_eq!(curve, "master");
        }
        other => panic!("expected CurvePointCountAnimatedWithPoints, got {other:?}"),
    }
    assert_eq!(
        mixed.to_string(),
        "effect \"color_curves\" curve \"master\" cannot keyframe point coordinates while its point_count has more than one keyframe"
    );
    recorded
        .push(json!({"case": "point_count animated with coordinates", "error": mixed.to_string()}));

    // The seventeenth managed colour node is a typed error.
    let mut crowded = cc3_document();
    for index in 0..COLOR_NODE_LIMIT_PER_LAYER {
        Operation::AddEffect {
            clip: ClipId(1),
            effect: wheels_effect(
                100 + index as u64,
                &[("gain_red_thousandths", 1_100 + index as i64)],
            ),
        }
        .apply(&mut crowded)
        .expect("the first sixteen managed nodes fit");
    }
    let full = crowded.clone();
    let overflow = Operation::AddEffect {
        clip: ClipId(1),
        effect: wheels_effect(999, &[("gain_red_thousandths", 1_200)]),
    }
    .apply(&mut crowded)
    .expect_err("the seventeenth managed node must be rejected");
    assert_eq!(crowded, full, "the overflow rejection must be atomic");
    match &overflow {
        OpError::TooManyColorNodes {
            clip,
            limit,
            actual,
        } => {
            assert_eq!(*clip, ClipId(1));
            assert_eq!((*limit, *actual), (COLOR_NODE_LIMIT_PER_LAYER, 17));
        }
        other => panic!("expected TooManyColorNodes, got {other:?}"),
    }
    assert_eq!(
        overflow.to_string(),
        "clip 1 would carry 17 managed colour nodes, exceeding the limit of 16"
    );
    recorded
        .push(json!({"case": "seventeenth managed colour node", "error": overflow.to_string()}));

    let metrics = json!({"rejections": recorded, "atomic": true});
    emit_cc3_evidence(
        "cc3_typed_rejections",
        "backend=kinewright_core_validation;adapter=host;software_fallback=true;gpu_claim=false;lane=cpu_reference",
        CPU_REFERENCE_LANE,
        json!({"rule": "10.1.6 field + observed + allowed"}),
        (0, 0),
        json_hash(&metrics),
        metrics,
    );
}

// ---------------------------------------------------------------------------
// §10.3.12: proof parity.
// ---------------------------------------------------------------------------

/// CC3 §10.3.12. A clip carrying all three node kinds renders identically
/// through the full-raster monitor proof and the production preview renderer,
/// and the ordered stage list matches `clip.effects`.
///
/// The proof *manifest* — the ordered stage names with their bypass flags and
/// resolved curve points — is assembled by `kinewright-agent`'s
/// `render_color_proof`, and `MonitorProofMetadata` (the only proof metadata
/// the media crate exposes) carries renderer provenance rather than a stage
/// list. This fixture therefore asserts everything media owns: identical
/// pixels, the CPU-reference monitor gate, honest adapter provenance, and the
/// serialized stage order that the agent manifest has to reproduce.
#[test]
fn cc3_proof_parity_renders_all_three_node_kinds_identically() {
    initialize_ffmpeg().expect("FFmpeg must initialize for the CC3 proof fixture");
    let directory = TempDirectory::new("cc3-monitor-proof");
    let width = 2_048_u32;
    let height = 2_u32;
    let (path, _source_bytes) = generate_delivery_source(&directory, width, height);
    let asset = probe_path(&path, AssetId(6)).expect("CC3 proof source should probe");
    assert_eq!(asset.resolution, Some((width, height)));
    let raw_description = asset.color_description.clone();
    let mut document = simple_document(asset, (width, height));
    document.tracks[0].clips[0].effects = vec![
        primary_effect(1),
        representative_wheels(2),
        representative_curves(3),
    ];
    document
        .validate()
        .expect("CC3 proof document must validate");
    let document = Arc::new(document);

    let stage_names = clip_effects(&document)
        .iter()
        .map(|effect| effect.name.clone())
        .collect::<Vec<_>>();
    assert_eq!(
        stage_names,
        vec![
            "primary_correction".to_owned(),
            "color_wheels".to_owned(),
            "color_curves".to_owned()
        ]
    );
    let evaluated = clip_effects(&document)
        .iter()
        .map(|effect| effect.evaluated_at(TimeCode::ZERO))
        .collect::<Vec<_>>();
    assert_eq!(
        active_color_nodes(&evaluated),
        vec![
            (0, ColorNodeKind::Primary),
            (1, ColorNodeKind::Wheels),
            (2, ColorNodeKind::Curves),
        ],
        "the ordered stage list must be the serialized clip.effects order"
    );
    for effect in &evaluated {
        assert_eq!(
            classify_color_node(effect).map(ColorNodeKind::effect_name),
            Some(effect.name.as_str()),
            "every stage must classify as its own managed node kind"
        );
    }

    let gpu = fallback_gpu();
    let engine = crate::engine::FfmpegMediaEngine::new_with_gpu(gpu.context())
        .expect("media engine should start on the fixture adapter");
    let proof = engine
        .monitor_proof_for_document(Arc::clone(&document), TimeCode::ZERO)
        .expect("full-raster monitor proof");
    let mut renderer = crate::render::FrameRenderer::new(gpu.context());
    let preview = renderer
        .render(
            &document,
            TimeCode::ZERO,
            (width, height),
            crate::render::RenderScale::FullResolution,
            crate::render::DecodeStrategy::Seek,
        )
        .expect("production full-raster preview render");
    let identical = preview.rgba.as_ref() == proof.image.pixels.as_slice();
    assert!(
        identical,
        "the CC3 monitor proof and the full-raster preview diverged"
    );

    let working = decode_managed_working_frame(&path, &raw_description);
    assert_eq!((working.width, working.height), (width, height));
    let nodes = cpu_nodes(&evaluated);
    assert_eq!(nodes.len(), 3);
    let cpu_reference = cpu_reference_monitor(&working, &nodes);
    let metric = abs_code_diff_rgb(&proof.image.pixels, &cpu_reference);
    assert!(
        metric.max <= MONITOR_CPU_GPU_MAX,
        "CC3 proof vs CPU reference max: {metric:?}"
    );
    assert!(
        metric.p99 <= MONITOR_CPU_GPU_P99,
        "CC3 proof vs CPU reference P99: {metric:?}"
    );
    assert!(
        metric.mean <= MONITOR_CPU_GPU_MEAN,
        "CC3 proof vs CPU reference mean: {metric:?}"
    );

    assert_eq!((proof.image.width, proof.image.height), (width, height));
    assert!(proof.metadata.full_resolution);
    assert!(matches!(
        proof.metadata.render_kind,
        kinewright_core::MonitorProofRenderKind::GpuPreview
    ));
    gpu.assert_proof_provenance(&proof.metadata);

    // The nodes must actually change the proof, or "renders identically"
    // would be a statement about a neutral pipeline.
    let neutral_reference = cpu_reference_monitor(&working, &[]);
    assert_ne!(
        cpu_reference, neutral_reference,
        "the three-node stack must move the proof raster"
    );

    let metrics = json!({
        "lane": gpu.lane.id(),
        "stage_names": stage_names,
        "stage_indices": [0, 1, 2],
        "proof_raster": [proof.image.width, proof.image.height],
        "same_render_semantics": identical,
        "cpu_reference_max_code_error": metric.max,
        "cpu_reference_p99_code_error": metric.p99,
        "cpu_reference_mean_code_error": metric.mean,
        "monitor_gate": {"max": MONITOR_CPU_GPU_MAX, "p99": MONITOR_CPU_GPU_P99, "mean": MONITOR_CPU_GPU_MEAN},
        "proof_metadata": proof.metadata,
        "ordered_stage_manifest_owner": "kinewright-agent render_color_proof",
    });
    emit_cc3_evidence(
        "cc3_proof_parity",
        gpu.backend(),
        gpu.lane.id(),
        json!({
            "nodes": ["primary_correction", "color_wheels", "color_curves"],
            "source_description_raw": raw_description,
            "source_profile": ColorSourceProfile::Rec709Video.id(),
        }),
        (width, height),
        output_hash(&proof.image.pixels),
        metrics,
    );
    println!("CC3_EVIDENCE_SOURCE {}", file_hash(&path));
}

// ---------------------------------------------------------------------------
// The fixture manifest.
// ---------------------------------------------------------------------------

/// CC3 §10.1.4. Every declared tolerance equals the code constant the fixtures
/// actually gate with, and every required fixture is declared.
#[test]
fn cc3_manifest_declares_every_required_fixture_and_tolerance() {
    let manifest: Value = serde_json::from_str(include_str!("../tests/fixtures/cc3_manifest.json"))
        .expect("CC3 fixture manifest must be valid JSON");
    assert_eq!(manifest["manifest_version"], 1);
    assert_eq!(manifest["contract"], "CC3 curves and wheels");
    assert_eq!(manifest["contract_token"], CC3_CONTRACT);
    assert_eq!(
        manifest["nodes"],
        json!(kinewright_core::MANAGED_COLOR_NODE_NAMES)
    );

    // §4.1 control table.
    let controls = manifest["wheels_controls"]
        .as_array()
        .expect("manifest must declare the wheels control table");
    assert_eq!(controls.len(), WHEEL_CONTROLS.len() + 1);
    for (declared, (name, minimum, maximum, neutral, _)) in controls.iter().zip(WHEEL_CONTROLS) {
        assert_eq!(declared["name"], name);
        assert_eq!(declared["min"], minimum);
        assert_eq!(declared["max"], maximum);
        assert_eq!(declared["neutral"], neutral);
    }
    let bypass = controls.last().expect("bypass row");
    assert_eq!(bypass["name"], "bypass");
    assert_eq!(
        (bypass["min"].as_i64(), bypass["max"].as_i64()),
        (Some(0), Some(1))
    );

    // §4.2 curve pattern.
    assert_eq!(manifest["curves"]["parameters"], 133);
    assert_eq!(manifest["curves"]["points_per_curve_max"], 16);
    assert_eq!(
        manifest["curves"]["coordinate_min"],
        COLOR_CURVE_COORDINATE_MIN
    );
    assert_eq!(
        manifest["curves"]["coordinate_max"],
        COLOR_CURVE_COORDINATE_MAX
    );

    // §10.2 raster.
    assert_eq!(manifest["raster"]["rgb_samples"], 192);
    assert_eq!(manifest["raster"]["patterns"], json!(CC3_PATTERNS));
    let levels = manifest["raster"]["levels"]
        .as_array()
        .expect("manifest must declare the raster levels");
    assert_eq!(levels.len(), CC3_RASTER_LEVELS.len());
    for (declared, expected) in levels.iter().zip(CC3_RASTER_LEVELS) {
        assert_eq!(
            declared.as_f64().expect("numeric raster level") as f32,
            expected,
            "manifest raster level does not match the code constant"
        );
    }

    // §10.1.4: tolerances are asserted equal to the code constants.
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
        "identity_ramp_monitor_max_code",
        f64::from(IDENTITY_RAMP_MONITOR_MAX),
    );
    assert_manifest_f64(
        tolerances,
        "identity_ramp_monitor_p99_code",
        IDENTITY_RAMP_MONITOR_P99,
    );
    assert_manifest_f64(
        tolerances,
        "identity_ramp_monitor_mean_code",
        IDENTITY_RAMP_MONITOR_MEAN,
    );
    assert_manifest_f32(tolerances, "anchor_tolerance", ANCHOR_TOLERANCE);
    assert_manifest_f64(
        tolerances,
        "spec_relative_tolerance",
        SPEC_RELATIVE_TOLERANCE,
    );
    assert_manifest_f64(tolerances, "spec_absolute_floor", SPEC_ABSOLUTE_FLOOR);
    assert_manifest_f64(
        tolerances,
        "minimum_changed_linear_basis_points",
        MIN_CHANGED_LINEAR_BASIS_POINTS as f64,
    );

    // Every required fixture is declared, and nothing is declared that the
    // suite does not actually emit.
    assert_eq!(
        manifest["required_evidence"],
        json!(CC3_EVIDENCE_FIXTURES),
        "the manifest evidence list must match the emitted fixture names exactly"
    );
    let items = manifest["required_fixtures"]
        .as_array()
        .expect("manifest must map the §10.3 items to test names");
    assert_eq!(items.len(), 12, "§10.3 lists twelve required fixtures");
    for (index, item) in items.iter().enumerate() {
        assert_eq!(item["item"], index + 1);
        assert!(
            item["tests"]
                .as_array()
                .is_some_and(|tests| !tests.is_empty()),
            "§10.3 item {} must name at least one test or its owning crate",
            index + 1
        );
    }

    for lane in ["software", "software_unavailable_opt_in", "hardware"] {
        assert!(
            manifest["gpu_contexts"][lane].is_string(),
            "the manifest must describe the {lane} GPU lane"
        );
    }
}
