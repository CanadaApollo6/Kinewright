//! Objective CC4 evidence fixtures for `docs/CC4-LOOK-MANAGEMENT.md` §10.
//!
//! These fixtures live inside the media crate for the same reason the CC1 and
//! CC3 fixtures do: the `Rgba16Float` working frame, the LUT atlas, and the
//! production compositor are internal seams, and the evidence has to exercise
//! the real GPU path rather than a public re-implementation of it.
//!
//! Every helper CC1 already owns — provenance, the banded §6.2 linear gate,
//! the monitor code metric, the evidence artefact writer — and every helper
//! CC3 owns — the §10.2 parity raster, the neutral ramps, the vacuity gate —
//! is reused from [`crate::cc1_fixtures`] and [`crate::cc3_fixtures`] rather
//! than duplicated, so a CC1 tolerance can never drift away from the CC4
//! fixture that claims to reuse it.
//!
//! Per CC4 §10.1 rule 1 no expected value in this file is obtained by calling
//! `Lut3d::lookup`, `LutNode::apply`, `apply_color_nodes_at`, the compositor, or
//! the shader. Expected values are either literal constants transcribed from
//! the contract tables or computed by the `spec_*_f64` functions below and in
//! [`crate::cc_fixture_support`], which are an independent f64 transcription
//! of §2.6 and §3.5.
//!
//! Per CC4 §10.1 rule 7 the precision gate uses a **non-dyadic** 33³ look
//! (a cross-talk matrix followed by a filmic S-curve, written out below), not
//! an identity lattice, so the lattice-precision claim is not vacuous.

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

use std::{collections::BTreeMap, fmt::Write as _, fs, path::Path, sync::Arc};

use half::f16;
use kinewright_core::{
    AutomationCurve, ClipId, ColorNodeInactiveReason, ColorNodeKind, ColorStage, Command, Core,
    Document, Effect, EffectId, Event, JournalCommand, Keyframe, KeyframeInterpolation,
    LUT_ASSET_ID_MAX, LUT_NODE_LIMIT_PER_LAYER, LutAsset, LutAssetId, LutAssetKind, LutAssetSource,
    LutAvailabilityKind, LutNodeParams, MANAGED_COLOR_NODE_NAMES, OpError, Operation, ParamValue,
    QaSeverity, TimeCode, active_color_nodes, color_node_inactive_reason, effect_descriptor,
    is_matte_parameter, qa_document, validate_lut_asset,
};
use serde_json::{Value, json};

use crate::{
    COMPOSITOR_LEGACY_LUT_SLOT, COMPOSITOR_LUT_ATLAS_SLOTS, COMPOSITOR_LUT_SLOTS_PER_LAYER,
    COMPOSITOR_REQUIRED_STORAGE_BUFFER_BINDING_SIZE,
    COMPOSITOR_REQUIRED_STORAGE_BUFFERS_PER_SHADER_STAGE, COMPOSITOR_REQUIRED_TEXTURE_DIMENSION_3D,
    Compositor,
    builtin_looks::{BUILTIN_LOOK_SHA256, BuiltinLook},
    cc_fixture_support::{
        apply_stack, clip_effects, cpu_nodes_with, cpu_reference_linear, cpu_reference_monitor,
        document_from, effect_with, gpu_linear, gpu_monitor, grade_header_word,
        spec_grade709_decode_f64, spec_grade709_encode_f64, spec_luma_f64, spec_sign_f64,
        with_parameter,
    },
    cc1_fixtures::{
        DiffMetrics, FixtureGpu, LINEAR_CPU_GPU_MAX, LINEAR_CPU_GPU_MEAN, LINEAR_CPU_GPU_P99,
        LINEAR_GATE_DOMAIN, LINEAR_GATE_IN_GAMUT, LINEAR_OVER_RANGE_MEAN, LINEAR_OVER_RANGE_P99,
        LinearParityMetrics, MIN_CHANGED_LINEAR_BASIS_POINTS, MONITOR_CPU_GPU_MAX,
        MONITOR_CPU_GPU_MEAN, MONITOR_CPU_GPU_P99, abs_code_diff_rgb, assert_linear_parity,
        assert_manifest_f32, assert_manifest_f64, backend_metadata, fallback_gpu, git_revision,
        hardware_gpu, linear_parity_metrics, output_hash, simple_document, working_frame,
        write_evidence_artefact,
    },
    cc3_fixtures::{
        CC3_PATTERNS, CC3_RASTER_BLOCK_WIDTH, CC3_RASTER_HEIGHT, CC3_RASTER_LEVELS,
        assert_case_is_not_vacuous, bits_of, cc3_parity_raster, cc3_raster_frame, descending_pairs,
        json_hash, neutral_ramp, primary_effect, representative_curves, representative_wheels,
    },
    color_pipeline::{ColorNode, LutInputEncoding},
    compositor::grade_buffer_bytes_with_luts,
    frame::WorkingFrame,
    lut::{
        CubeLut, LutParseError, LutParseErrorCode, MAX_CUBE_SIZE, MIN_CUBE_SIZE,
        parse_cube_lut_bytes, parse_cube_lut_typed,
    },
    lut_store::{LUT_MAX_FILE_BYTES, LutLibrary, LutStore, metadata_mismatch},
    sha256::sha256_bytes,
    test_support::TempDirectory,
};

/// The contract token recorded on every CC4 evidence payload.
const CC4_CONTRACT: &str = "cc4_look_management";

/// Non-GPU fixtures still record a backend so a reader never has to guess
/// which implementation produced a number.
const CPU_REFERENCE_BACKEND: &str = "backend=kinewright_media_cpu_reference;adapter=host_f32;\
software_fallback=true;gpu_claim=false;lane=cpu_reference";
const CPU_REFERENCE_LANE: &str = "cpu_reference";

/// CC4 §10.3.10's gate: a built-in node reproduces its closed form to within
/// `2e-6` in **display code**. The contract measures `4.85e-7`; this is the
/// number the contract itself states, not a fixture invention.
const BUILTIN_DISPLAY_CODE_TOLERANCE: f64 = 2.0e-6;

/// CC4 §10.2: at least this many of the 192 raster samples must encode outside
/// `[0, 1]` in `display709`, so the §3.5 out-of-domain rule is exercised
/// non-vacuously. The contract measures 72.
const MIN_OUT_OF_DOMAIN_RASTER_SAMPLES: usize = 40;

/// The §3.5 anchor rows are exact binary fractions, so the CPU assertions are
/// exact equalities and the GPU ones use the CC1 §6.2 linear gate.
const ANCHOR_GATE: f32 = LINEAR_CPU_GPU_MAX;

/// The lattice edge of the non-dyadic parity look (CC4 §10.1 rule 7).
const NON_DYADIC_LOOK_SIZE: u32 = 33;

/// Every evidence payload this suite emits. The manifest is asserted equal to
/// this list, so a fixture cannot be deleted without the manifest test failing.
const CC4_EVIDENCE_FIXTURES: [&str; 17] = [
    "cc4_parsing",
    "cc4_input_encodings",
    "cc4_identity_bit_exact",
    "cc4_identity_encodings",
    "cc4_inactive_nodes",
    "cc4_interpolation_anchors",
    "cc4_out_of_domain",
    "cc4_mix",
    "cc4_node_ordering",
    "cc4_gpu_cpu_parity",
    "cc4_slots_and_limits",
    "cc4_legacy_coexistence",
    "cc4_builtin_bakes",
    "cc4_relocatable_store",
    "cc4_recovery_rejections",
    "cc4_serialization_history",
    "cc4_typed_rejections",
];

// ---------------------------------------------------------------------------
// The independent f64 transcription of CC4 §2.6, §3.4, and §3.5.
//
// Nothing below calls the production evaluator. The CC3 §2.1 `grade709`
// digits and pair, `sgn`, and the §2.6 luma are the hand transcription in
// `crate::cc_fixture_support`; the algorithms below are the §3.5 pseudocode,
// transcribed by hand, so a parity or anchor assertion compares two
// implementations of the written contract rather than one implementation with
// itself.
// ---------------------------------------------------------------------------

/// CC1's sign-preserving `encode_bt709`, in f64.
fn spec_encode_bt709_signed_f64(x: f64) -> f64 {
    let sign = spec_sign_f64(x);
    let magnitude = x.abs();
    if magnitude < 0.018 {
        sign * 4.5 * magnitude
    } else {
        sign * (1.099 * magnitude.powf(0.45) - 0.099)
    }
}

/// CC4 §3.4's `decode_display709`, the exact sign-preserving inverse, in f64.
fn spec_decode_display709_f64(e: f64) -> f64 {
    let sign = spec_sign_f64(e);
    let magnitude = e.abs();
    if magnitude < 0.081 {
        sign * magnitude / 4.5
    } else {
        sign * ((magnitude + 0.099) / 1.099).powf(1.0 / 0.45)
    }
}

/// `ENC` for one CC4 §3.4 token, in f64.
fn spec_encode_f64(encoding: LutInputEncoding, x: f64) -> f64 {
    match encoding {
        LutInputEncoding::Display709 => spec_encode_bt709_signed_f64(x),
        LutInputEncoding::Linear => x,
        LutInputEncoding::Grade709 => spec_grade709_encode_f64(x),
    }
}

/// `DEC` for one CC4 §3.4 token, in f64.
fn spec_decode_f64(encoding: LutInputEncoding, e: f64) -> f64 {
    match encoding {
        LutInputEncoding::Display709 => spec_decode_display709_f64(e),
        LutInputEncoding::Linear => e,
        LutInputEncoding::Grade709 => spec_grade709_decode_f64(e),
    }
}

/// A hand-transcribed lattice for the f64 reference evaluator.
///
/// Deliberately *not* [`crate::color_pipeline::Lut3d`]: CC4 §4.4 and §10.1
/// rule 1 require the fixture's expected values to come from a second,
/// independent transcription of §3.5.
#[derive(Debug, Clone)]
struct SpecLattice {
    size: u32,
    domain_min: f64,
    domain_max: f64,
    /// `S^3` RGB triples, red-fastest IRIDAS order.
    samples: Vec<[f64; 3]>,
}

impl SpecLattice {
    /// Build the lattice by evaluating `sample` at each lattice *coordinate*.
    fn new(size: u32, domain: (f64, f64), sample: impl Fn([f64; 3]) -> [f64; 3]) -> Self {
        let (domain_min, domain_max) = domain;
        let last = f64::from(size - 1);
        let mut samples = Vec::with_capacity((size as usize).pow(3));
        for blue in 0..size {
            for green in 0..size {
                for red in 0..size {
                    let coordinate = |index: u32| {
                        domain_min + (domain_max - domain_min) * (f64::from(index) / last)
                    };
                    samples.push(sample([
                        coordinate(red),
                        coordinate(green),
                        coordinate(blue),
                    ]));
                }
            }
        }
        Self {
            size,
            domain_min,
            domain_max,
            samples,
        }
    }

    /// Round every sample through the `{:.6}` decimal grid the canonical
    /// serializer writes and the parser reads back, and through `f32`, so the
    /// f64 reference evaluates exactly the lattice the renderer verified.
    fn quantized_like_cube_text(&self) -> Self {
        let samples = self
            .samples
            .iter()
            .map(|rgb| rgb.map(|value| f64::from(format_six(value).parse::<f32>().unwrap_or(0.0))))
            .collect();
        Self {
            samples,
            ..self.clone()
        }
    }

    /// `V(i_r, i_g, i_b)`, red-fastest.
    fn lattice(&self, i_r: u32, i_g: u32, i_b: u32) -> [f64; 3] {
        let last = self.size.saturating_sub(1);
        let (i_r, i_g, i_b) = (i_r.min(last), i_g.min(last), i_b.min(last));
        let size = self.size as usize;
        let index = ((i_b as usize * size) + i_g as usize) * size + i_r as usize;
        self.samples[index]
    }

    /// CC4 §3.5's tetrahedral lookup with the additive out-of-domain rule,
    /// transcribed verbatim from the contract's branch structure.
    fn lookup(&self, e: [f64; 3]) -> [f64; 3] {
        let last_index = self.size.saturating_sub(2);
        let span = f64::from(self.size - 1);
        let mut u = [0.0_f64; 3];
        let mut i = [0_u32; 3];
        let mut f = [0.0_f64; 3];
        for channel in 0..3 {
            let clamped = e[channel].clamp(self.domain_min, self.domain_max);
            let t = (clamped - self.domain_min) / (self.domain_max - self.domain_min);
            let s = t * span;
            let index = (s.floor() as u32).min(last_index);
            u[channel] = clamped;
            i[channel] = index;
            f[channel] = s - f64::from(index);
        }
        let (f_r, f_g, f_b) = (f[0], f[1], f[2]);
        let corner =
            |d_r: u32, d_g: u32, d_b: u32| self.lattice(i[0] + d_r, i[1] + d_g, i[2] + d_b);
        let c000 = corner(0, 0, 0);
        let y = if f_r > f_g {
            if f_g > f_b {
                spec_tetra(
                    c000,
                    f_r,
                    corner(1, 0, 0),
                    c000,
                    f_g,
                    corner(1, 1, 0),
                    corner(1, 0, 0),
                    f_b,
                    corner(1, 1, 1),
                    corner(1, 1, 0),
                )
            } else if f_r > f_b {
                spec_tetra(
                    c000,
                    f_r,
                    corner(1, 0, 0),
                    c000,
                    f_g,
                    corner(1, 1, 1),
                    corner(1, 0, 1),
                    f_b,
                    corner(1, 0, 1),
                    corner(1, 0, 0),
                )
            } else {
                spec_tetra(
                    c000,
                    f_r,
                    corner(1, 0, 1),
                    corner(0, 0, 1),
                    f_g,
                    corner(1, 1, 1),
                    corner(1, 0, 1),
                    f_b,
                    corner(0, 0, 1),
                    c000,
                )
            }
        } else if f_b > f_g {
            spec_tetra(
                c000,
                f_r,
                corner(1, 1, 1),
                corner(0, 1, 1),
                f_g,
                corner(0, 1, 1),
                corner(0, 0, 1),
                f_b,
                corner(0, 0, 1),
                c000,
            )
        } else if f_b > f_r {
            spec_tetra(
                c000,
                f_r,
                corner(1, 1, 1),
                corner(0, 1, 1),
                f_g,
                corner(0, 1, 0),
                c000,
                f_b,
                corner(0, 1, 1),
                corner(0, 1, 0),
            )
        } else {
            spec_tetra(
                c000,
                f_r,
                corner(1, 1, 0),
                corner(0, 1, 0),
                f_g,
                corner(0, 1, 0),
                c000,
                f_b,
                corner(1, 1, 1),
                corner(1, 1, 0),
            )
        };
        [
            y[0] + (e[0] - u[0]),
            y[1] + (e[1] - u[1]),
            y[2] + (e[2] - u[2]),
        ]
    }

    /// One whole CC4 §3.5 node: `ENC`, lookup, `DEC`, linear-light mix.
    fn apply(&self, encoding: LutInputEncoding, mix: f64, x: [f64; 3]) -> [f64; 3] {
        let e = x.map(|value| spec_encode_f64(encoding, value));
        let z = self.lookup(e);
        let looked = z.map(|value| spec_decode_f64(encoding, value));
        [
            x[0] + (looked[0] - x[0]) * mix,
            x[1] + (looked[1] - x[1]) * mix,
            x[2] + (looked[2] - x[2]) * mix,
        ]
    }
}

/// `base + f_r*(a1 - a0) + f_g*(b1 - b0) + f_b*(d1 - d0)`, the shared shape of
/// all six CC4 §3.5 formulas.
fn spec_tetra(
    base: [f64; 3],
    f_r: f64,
    a1: [f64; 3],
    a0: [f64; 3],
    f_g: f64,
    b1: [f64; 3],
    b0: [f64; 3],
    f_b: f64,
    d1: [f64; 3],
    d0: [f64; 3],
) -> [f64; 3] {
    let mut out = [0.0_f64; 3];
    for channel in 0..3 {
        out[channel] = base[channel]
            + f_r * (a1[channel] - a0[channel])
            + f_g * (b1[channel] - b0[channel])
            + f_b * (d1[channel] - d0[channel]);
    }
    out
}

/// The five §2.6 built-in formulas, transcribed independently of
/// [`BuiltinLook::formula`]. `cc4_builtin_bakes_are_deterministic_and_match_their_formulas`
/// asserts the two agree, so the fixture never silently adopts the production
/// spelling of the contract.
fn spec_builtin_formula_f64(look: BuiltinLook, e: [f64; 3]) -> [f64; 3] {
    match look {
        BuiltinLook::Identity => e,
        BuiltinLook::Warm => [
            (e[0] - 0.5) * 1.08 + 0.54,
            (e[1] - 0.5) * 1.08 + 0.50,
            (e[2] - 0.5) * 1.08 + 0.46,
        ],
        BuiltinLook::Cool => [
            (e[0] - 0.5) * 1.12 + 0.46,
            (e[1] - 0.5) * 1.12 + 0.50,
            (e[2] - 0.5) * 1.12 + 0.55,
        ],
        BuiltinLook::Monochrome => {
            let luma = spec_luma_f64(e);
            [luma, luma, luma]
        }
        BuiltinLook::BleachBypass => {
            let luma = spec_luma_f64(e);
            let mixed = [
                luma + (e[0] - luma) * 0.35,
                luma + (e[1] - luma) * 0.35,
                luma + (e[2] - luma) * 0.35,
            ];
            [
                (mixed[0] - 0.5) * 1.35 + 0.5,
                (mixed[1] - 0.5) * 1.35 + 0.5,
                (mixed[2] - 0.5) * 1.35 + 0.5,
            ]
        }
    }
}

// ---------------------------------------------------------------------------
// The fixture lattices, written as real `.cube` text and imported through the
// real project store (CC4 §2.4: the renderer only ever consumes hash-verified
// bytes, so a fixture that fabricated a `LutLibrary` from samples would not be
// testing the production path).
// ---------------------------------------------------------------------------

/// The canonical serializer's fixed-decimal spelling.
fn format_six(value: f64) -> String {
    format!("{value:.6}")
}

/// Serialize a lattice as `.cube` text, red-fastest, with `{:.6}` decimals.
fn cube_text(size: u32, domain: (f64, f64), sample: impl Fn(u32, u32, u32) -> [f64; 3]) -> String {
    let minimum = format_six(domain.0);
    let maximum = format_six(domain.1);
    let mut text = String::new();
    let _ = writeln!(text, "LUT_3D_SIZE {size}");
    let _ = writeln!(text, "DOMAIN_MIN {minimum} {minimum} {minimum}");
    let _ = writeln!(text, "DOMAIN_MAX {maximum} {maximum} {maximum}");
    for blue in 0..size {
        for green in 0..size {
            for red in 0..size {
                let rgb = sample(red, green, blue);
                let _ = writeln!(
                    text,
                    "{} {} {}",
                    format_six(rgb[0]),
                    format_six(rgb[1]),
                    format_six(rgb[2])
                );
            }
        }
    }
    text
}

/// Serialize a [`SpecLattice`] as `.cube` text.
fn lattice_cube_text(lattice: &SpecLattice) -> String {
    cube_text(
        lattice.size,
        (lattice.domain_min, lattice.domain_max),
        |red, green, blue| lattice.lattice(red, green, blue),
    )
}

/// The `S`-edge identity lattice over `[0, 1]` (CC4 §10.3.2a).
///
/// `S - 1` is a power of two for every `S` the fixture uses, so every lattice
/// value is an exact binary fraction *and* an exact six-decimal string.
fn identity_lattice(size: u32) -> SpecLattice {
    SpecLattice::new(size, (0.0, 1.0), |e| e)
}

/// CC4 §10.3.3 LUT B: `S = 2` over `[0, 1]`, written out corner by corner.
fn lut_b_lattice() -> SpecLattice {
    const CORNERS: [[f64; 3]; 8] = [
        [0.0, 0.0, 0.0],
        [0.5, 0.0, 0.0],
        [0.0, 0.5, 0.0],
        [0.5, 0.5, 0.0],
        [0.0, 0.0, 0.5],
        [0.5, 0.0, 0.5],
        [0.0, 0.5, 0.5],
        [1.0, 1.0, 1.0],
    ];
    SpecLattice {
        size: 2,
        domain_min: 0.0,
        domain_max: 1.0,
        samples: CORNERS.to_vec(),
    }
}

/// The separable `f = (0, 0.25, 1.0)` lattice CC4 §10.3.3 defines for LUT C
/// and LUT D.
fn separable_lattice(domain: (f64, f64)) -> SpecLattice {
    const F: [f64; 3] = [0.0, 0.25, 1.0];
    let mut samples = Vec::with_capacity(27);
    for blue in F {
        for green in F {
            for red in F {
                samples.push([red, green, blue]);
            }
        }
    }
    SpecLattice {
        size: 3,
        domain_min: domain.0,
        domain_max: domain.1,
        samples,
    }
}

/// LUT C: the separable lattice over `[0, 1]`.
fn lut_c_lattice() -> SpecLattice {
    separable_lattice((0.0, 1.0))
}

/// LUT D: the same lattice over `[-0.5, 1.5]`, the domain-mapping anchor.
fn lut_d_lattice() -> SpecLattice {
    separable_lattice((-0.5, 1.5))
}

/// The CC4 §10.1 rule 7 cross-talk matrix. Every row sums to exactly `1`, so
/// an in-domain input maps into `[0, 1]` before the S-curve.
const LOOK_CROSSTALK: [[f64; 3]; 3] = [[0.88, 0.08, 0.04], [0.06, 0.90, 0.04], [0.05, 0.07, 0.88]];

/// The filmic S-curve of the non-dyadic parity look, written out here.
///
/// `s(t) = (t * (2.51 t + 0.03)) / (t * (2.43 t + 0.59) + 0.14)`; the
/// denominator is `>= 0.14` for `t` in `[0, 1]`, so no lattice sample is
/// non-finite and none is a dyadic rational.
fn spec_filmic_f64(t: f64) -> f64 {
    (t * (2.51 * t + 0.03)) / (t * (2.43 * t + 0.59) + 0.14)
}

/// The non-dyadic 33³ creative look CC4 §10.1 rule 7 requires.
fn spec_non_dyadic_look_f64(e: [f64; 3]) -> [f64; 3] {
    let mixed = LOOK_CROSSTALK.map(|row| row[0] * e[0] + row[1] * e[1] + row[2] * e[2]);
    mixed.map(spec_filmic_f64)
}

/// The non-dyadic parity look as a lattice.
fn non_dyadic_look_lattice() -> SpecLattice {
    SpecLattice::new(NON_DYADIC_LOOK_SIZE, (0.0, 1.0), spec_non_dyadic_look_f64)
}

// ---------------------------------------------------------------------------
// The store-backed library.
// ---------------------------------------------------------------------------

/// A real project store holding real `.cube` files, plus the verified library
/// the renderer consumes.
struct FixtureLuts {
    directory: TempDirectory,
    assets: Vec<LutAsset>,
    library: LutLibrary,
}

impl FixtureLuts {
    /// Import each `.cube` text in order, allocating ids `1 ..= n`.
    fn build(label: &str, sources: &[String]) -> Self {
        let directory = TempDirectory::new(label);
        let store = LutStore::for_project(&directory.path("project.kinewright"))
            .expect("a temporary project path derives a store root");
        let mut assets = Vec::with_capacity(sources.len());
        for (index, text) in sources.iter().enumerate() {
            let source = directory.path(&format!("look-{index}.cube"));
            fs::write(&source, text).expect("the fixture LUT is written");
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
        assert_eq!(library.len(), sources.len());
        Self {
            directory,
            assets,
            library,
        }
    }

    /// One lattice's fixture: import it and return the library.
    fn one(label: &str, lattice: &SpecLattice) -> Self {
        Self::build(label, &[lattice_cube_text(lattice)])
    }

    fn library(&self) -> &LutLibrary {
        &self.library
    }

    fn asset(&self, index: usize) -> &LutAsset {
        &self.assets[index]
    }

    fn directory(&self) -> &TempDirectory {
        &self.directory
    }

    /// A `Document` carrying every imported asset, for the operation fixtures.
    fn document(&self) -> Document {
        let mut document = cc4_document();
        document.lut_assets = self.assets.clone();
        document
    }
}

// ---------------------------------------------------------------------------
// Effects.
// ---------------------------------------------------------------------------

/// A `creative_look` node bound to `asset`, at `mix` basis points and the
/// given `input_encoding_token`.
fn creative_look(id: u64, asset: i64, encoding: i64, mix: i64) -> Effect {
    effect_with(
        id,
        "creative_look",
        &[
            ("lut_asset_id", asset),
            ("mix_basis_points", mix),
            ("input_encoding_token", encoding),
        ],
    )
}

/// A `technical_lut` node bound to `asset`. `mix_basis_points` is pinned by
/// its descriptor, so it is not written here.
fn technical_lut(id: u64, asset: i64, encoding: i64) -> Effect {
    effect_with(
        id,
        "technical_lut",
        &[("lut_asset_id", asset), ("input_encoding_token", encoding)],
    )
}

fn cc4_asset() -> kinewright_core::MediaAsset {
    kinewright_core::MediaAsset {
        id: kinewright_core::AssetId(1),
        path: std::path::PathBuf::from("cc4-fixture.mp4"),
        name: "cc4 fixture".to_owned(),
        duration: TimeCode(30),
        fps: kinewright_core::Rational::new(30, 1).expect("cc4 fixture fps"),
        kind: kinewright_core::MediaKind::Video,
        resolution: Some((16, 16)),
        source_fingerprint: kinewright_core::MediaSourceFingerprint::default(),
        color_description: kinewright_core::ColorDescription::default(),
    }
}

fn cc4_document() -> Document {
    simple_document(cc4_asset(), (16, 16))
}

// ---------------------------------------------------------------------------
// CPU reference and GPU rendering.
// ---------------------------------------------------------------------------

/// Render one case on the GPU, compare it against the CPU reference, and apply
/// the CC1 §6.2 gates verbatim.
fn assert_gpu_case_with_luts(
    compositor: &Compositor,
    resolution: (u32, u32),
    frame: &WorkingFrame,
    effects: &[Effect],
    library: &LutLibrary,
    baseline_linear: Option<&[f32]>,
    label: &str,
) -> (DiffMetrics, LinearParityMetrics, Vec<u8>) {
    let nodes = cpu_nodes_with(effects, library);
    let expected_linear = cpu_reference_linear(frame, &nodes);
    let expected_monitor = cpu_reference_monitor(frame, &nodes);
    if let Some(baseline) = baseline_linear {
        assert_case_is_not_vacuous(&expected_linear, baseline, label);
    }
    let actual_linear = gpu_linear(compositor, resolution, frame, effects, Some(library));
    let actual_monitor = gpu_monitor(compositor, resolution, frame, effects, Some(library));
    let linear = linear_parity_metrics(&actual_linear, &expected_linear);
    let monitor = abs_code_diff_rgb(&actual_monitor, &expected_monitor);
    assert!(
        linear.in_gamut_samples > 0,
        "case {label} left the in-gamut §6.2 band empty, so the linear gate was never applied: {linear:?}"
    );
    assert_eq!(
        linear.non_finite, 0,
        "case {label} produced a non-finite linear sample: {linear:?}"
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
// The CC4 §4.2 storage-buffer reader, so slot assignment is checked against
// the bytes the shader actually reads.
// ---------------------------------------------------------------------------

const GRADE_HEADER_BYTES: usize = 16;
const GRADE_NODE_WORDS: usize = 16;
const GRADE_NODE_VALUE_OFFSET: usize = 4;

fn grade_value(bytes: &[u8], node: usize, value: usize) -> f32 {
    let word = node * GRADE_NODE_WORDS + GRADE_NODE_VALUE_OFFSET + value;
    let offset = GRADE_HEADER_BYTES + word * 4;
    f32::from_le_bytes(
        bytes[offset..offset + 4]
            .try_into()
            .expect("f32-aligned node word"),
    )
}

fn grade_kind(bytes: &[u8], node: usize) -> u32 {
    let offset = GRADE_HEADER_BYTES + node * GRADE_NODE_WORDS * 4;
    f32::from_le_bytes(bytes[offset..offset + 4].try_into().expect("kind word")) as u32
}

// ---------------------------------------------------------------------------
// Evidence.
// ---------------------------------------------------------------------------

fn emit_cc4_evidence(
    fixture: &str,
    backend: &str,
    lane: &str,
    controls: Value,
    raster: (u32, u32),
    output_hash: String,
    metrics: Value,
) {
    assert!(
        CC4_EVIDENCE_FIXTURES.contains(&fixture),
        "every CC4 evidence payload must be declared in CC4_EVIDENCE_FIXTURES and in the manifest; {fixture} is not"
    );
    let provenance = backend_metadata(backend);
    let field = |key: &str| provenance.get(key).cloned().unwrap_or(Value::Null);
    let payload = json!({
        "contract": CC4_CONTRACT,
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
    println!("CC4_EVIDENCE {payload}");
    write_evidence_artefact(fixture, &payload);
}

// ---------------------------------------------------------------------------
// §10.3.1: `.cube` parsing.
// ---------------------------------------------------------------------------

/// A minimal well-formed `S = 2` body, so a fault can be injected one line at
/// a time without the rest of the file changing.
fn minimal_cube_body() -> String {
    let mut text = String::new();
    for blue in 0..2 {
        for green in 0..2 {
            for red in 0..2 {
                let _ = writeln!(
                    text,
                    "{} {} {}",
                    format_six(f64::from(red)),
                    format_six(f64::from(green)),
                    format_six(f64::from(blue))
                );
            }
        }
    }
    text
}

/// CC4 §10.3.1. Every documented acceptance form parses, and every documented
/// rejection reports its code, 1-based line, `observed`, and `allowed`.
#[test]
fn cc4_cube_parsing_accepts_and_rejects_exactly_what_the_contract_lists() {
    assert_eq!(MIN_CUBE_SIZE, 2, "CC4 §2.1 fixes the smallest lattice edge");
    assert_eq!(
        MAX_CUBE_SIZE, 65,
        "CC4 §2.5 raises the largest lattice edge from 64 to 65"
    );

    // --- accepted sizes -------------------------------------------------
    let mut accepted_sizes = Vec::new();
    for size in [2_u32, 17, 33, 65] {
        let lattice = identity_lattice(size);
        let text = lattice_cube_text(&lattice);
        let parsed = parse_cube_lut_typed(&text)
            .unwrap_or_else(|error| panic!("size {size} must parse: {error}"));
        assert_eq!(parsed.size, size);
        assert_eq!(parsed.domain_min, [0.0; 3]);
        assert_eq!(parsed.domain_max, [1.0; 3]);
        assert_eq!(parsed.sample_count(), 3 * (size as usize).pow(3));
        // `S - 1` is a power of two for every accepted size, so the identity
        // lattice values are exact binary fractions and exact six-decimal
        // strings; the last red-fastest corner is (1, 1, 1) exactly.
        let last = (size as usize).pow(3) - 1;
        assert_eq!(parsed.sample(0), Some([0.0, 0.0, 0.0]));
        assert_eq!(parsed.sample(last), Some([1.0, 1.0, 1.0]));
        assert_eq!(
            parsed.sample(1),
            Some([1.0 / f32::from(size as u16 - 1), 0.0, 0.0])
        );
        accepted_sizes.push(size);
    }

    // --- negative domain ------------------------------------------------
    let negative_domain = lattice_cube_text(&lut_d_lattice());
    let parsed = parse_cube_lut_typed(&negative_domain).expect("a negative domain is legal");
    assert_eq!(parsed.domain_min, [-0.5; 3]);
    assert_eq!(parsed.domain_max, [1.5; 3]);
    assert_eq!(
        parsed.domain_millionths(),
        ([-500_000; 3], [1_500_000; 3]),
        "the integer mirrors round half away from zero"
    );

    // --- quoted title, comments, blank lines, lowercase keywords,
    //     scientific notation ------------------------------------------
    let decorated = "\
# a leading comment
title  \"Kodak 2383 D65\"

lut_3d_size 2
# the domain, in scientific notation
domain_min -1e-1 -1.0E-1 -0.1
domain_max 1.5e0 1.5 1.5

0 0 0     # the first corner
1e0 0 0
0 5E-1 0
1 0.5 0
0 0 2.5e-1
1 0 0.25
0 0.5 0.25
1 1 1
";
    let parsed = parse_cube_lut_typed(decorated).expect("every decorated form is accepted");
    assert_eq!(parsed.size, 2);
    assert_eq!(parsed.title.as_deref(), Some("Kodak 2383 D65"));
    assert_eq!(parsed.domain_min, [-0.1; 3]);
    assert_eq!(parsed.domain_max, [1.5; 3]);
    assert_eq!(parsed.sample(3), Some([1.0, 0.5, 0.0]));
    assert_eq!(parsed.sample(4), Some([0.0, 0.0, 0.25]));

    // --- CRLF -----------------------------------------------------------
    let crlf = decorated.replace('\n', "\r\n");
    let crlf_parsed = parse_cube_lut_typed(&crlf).expect("CRLF is accepted");
    assert_eq!(crlf_parsed, parsed, "CRLF must parse identically to LF");

    // --- UTF-8 BOM ------------------------------------------------------
    let mut bom = vec![0xEF_u8, 0xBB, 0xBF];
    bom.extend_from_slice(decorated.as_bytes());
    let bom_parsed = parse_cube_lut_bytes(&bom).expect("a leading BOM is stripped");
    assert_eq!(bom_parsed, parsed);

    // --- rejections -----------------------------------------------------
    let body = minimal_cube_body();
    let with_size = |head: &str| format!("{head}\n{body}");
    let mut rejections = Vec::new();
    let mut check = |label: &str, source: String, expected: LutParseError| {
        let error = parse_cube_lut_typed(&source)
            .map(|_| ())
            .expect_err(&format!("{label} must be rejected"));
        assert_eq!(error.code, expected.code, "{label}: code");
        assert_eq!(error.line, expected.line, "{label}: 1-based line");
        assert_eq!(error.observed, expected.observed, "{label}: observed");
        assert_eq!(error.allowed, expected.allowed, "{label}: allowed");
        rejections.push(json!({
            "case": label,
            "code": error.code.as_str(),
            "line": error.line,
            "observed": error.observed,
            "allowed": error.allowed,
        }));
    };

    check(
        "lut_1d_size",
        "LUT_1D_SIZE 32\n0 0 0\n".to_owned(),
        LutParseError {
            code: LutParseErrorCode::UnsupportedLutFormat,
            line: Some(1),
            observed: "LUT_1D_SIZE 32".to_owned(),
            allowed: "a 3D .cube LUT declared with LUT_3D_SIZE".to_owned(),
        },
    );
    check(
        "size_1",
        with_size("LUT_3D_SIZE 1"),
        LutParseError {
            code: LutParseErrorCode::LutSizeOutOfRange,
            line: Some(1),
            observed: "1".to_owned(),
            allowed: "an integer in 2..=65".to_owned(),
        },
    );
    check(
        "size_66",
        with_size("LUT_3D_SIZE 66"),
        LutParseError {
            code: LutParseErrorCode::LutSizeOutOfRange,
            line: Some(1),
            observed: "66".to_owned(),
            allowed: "an integer in 2..=65".to_owned(),
        },
    );
    check(
        "two_value_data_line",
        format!("LUT_3D_SIZE 2\n0.000000 0.000000\n{body}"),
        LutParseError {
            code: LutParseErrorCode::MalformedLutFile,
            line: Some(2),
            observed: "0.000000 0.000000".to_owned(),
            allowed: "exactly three whitespace-separated sample values".to_owned(),
        },
    );
    check(
        "repeated_lut_3d_size",
        format!("LUT_3D_SIZE 2\nLUT_3D_SIZE 2\n{body}"),
        LutParseError {
            code: LutParseErrorCode::MalformedLutFile,
            line: Some(2),
            observed: "LUT_3D_SIZE 2".to_owned(),
            allowed: "exactly one LUT_3D_SIZE keyword, before any sample line".to_owned(),
        },
    );
    check(
        "domain_min_equals_domain_max",
        format!("LUT_3D_SIZE 2\nDOMAIN_MIN 1 1 1\nDOMAIN_MAX 1 1 1\n{body}"),
        LutParseError {
            code: LutParseErrorCode::LutDomainInvalid,
            // The reported line is the last domain keyword the parser saw,
            // because the comparison is only possible once both are known.
            line: Some(3),
            observed: "channel 0: DOMAIN_MIN 1, DOMAIN_MAX 1".to_owned(),
            allowed: "DOMAIN_MIN strictly less than DOMAIN_MAX on every channel".to_owned(),
        },
    );
    // A data line carries exactly three values or it is `malformed_lut_file`,
    // so the reachable count mismatch is one whole triple short or long — the
    // `3 * S^3 ± 1` shape the contract names, at the granularity the grammar
    // permits.
    let mut short = body.clone();
    short.truncate(
        short
            .rfind('\n')
            .and_then(|end| short[..end].rfind('\n'))
            .unwrap_or(0)
            + 1,
    );
    check(
        "sample_count_short",
        format!("LUT_3D_SIZE 2\n{short}"),
        LutParseError {
            code: LutParseErrorCode::LutSampleCountMismatch,
            line: None,
            observed: "21".to_owned(),
            allowed: "24 scalar samples (3 * 2^3)".to_owned(),
        },
    );
    check(
        "sample_count_long",
        format!("LUT_3D_SIZE 2\n{body}1.000000 1.000000 1.000000\n"),
        LutParseError {
            code: LutParseErrorCode::LutSampleCountMismatch,
            line: None,
            observed: "27".to_owned(),
            allowed: "24 scalar samples (3 * 2^3)".to_owned(),
        },
    );
    check(
        "sample_nan",
        format!("LUT_3D_SIZE 2\n0.0 NaN 0.0\n{body}"),
        LutParseError {
            code: LutParseErrorCode::LutSampleNotFinite,
            line: Some(2),
            observed: "NaN".to_owned(),
            allowed: "a finite sample value".to_owned(),
        },
    );
    check(
        "sample_infinity",
        format!("LUT_3D_SIZE 2\n0.0 0.0 inf\n{body}"),
        LutParseError {
            code: LutParseErrorCode::LutSampleNotFinite,
            line: Some(2),
            observed: "inf".to_owned(),
            allowed: "a finite sample value".to_owned(),
        },
    );
    check(
        "domain_not_finite",
        format!("LUT_3D_SIZE 2\nDOMAIN_MAX inf inf inf\n{body}"),
        LutParseError {
            code: LutParseErrorCode::LutDomainInvalid,
            line: Some(2),
            observed: "inf".to_owned(),
            allowed: "a finite domain bound".to_owned(),
        },
    );
    check(
        "missing_lut_3d_size",
        body.clone(),
        LutParseError {
            code: LutParseErrorCode::MalformedLutFile,
            line: Some(1),
            observed: "0.000000 0.000000 0.000000".to_owned(),
            allowed: "LUT_3D_SIZE before the first sample line".to_owned(),
        },
    );

    // Non-UTF-8 bytes are the one fault only the byte entry point can see.
    let mut invalid = b"LUT_3D_SIZE 2\n".to_vec();
    invalid.push(0xFF);
    invalid.extend_from_slice(b"\n");
    let error = parse_cube_lut_bytes(&invalid).expect_err("non-UTF-8 bytes must be rejected");
    assert_eq!(error.code, LutParseErrorCode::MalformedLutFile);
    assert_eq!(error.line, Some(2));
    assert_eq!(error.observed, "a non-UTF-8 byte at offset 14");
    assert_eq!(error.allowed, "UTF-8 text");
    rejections.push(json!({
        "case": "non_utf8",
        "code": error.code.as_str(),
        "line": error.line,
        "observed": error.observed,
        "allowed": error.allowed,
    }));

    let metrics = json!({
        "accepted_sizes": accepted_sizes,
        "accepted_forms": [
            "quoted TITLE", "# comments", "blank lines", "CRLF",
            "lowercase keywords", "scientific notation", "UTF-8 BOM",
            "negative DOMAIN_MIN",
        ],
        "rejections": rejections,
        "max_file_bytes": LUT_MAX_FILE_BYTES,
        "min_cube_size": MIN_CUBE_SIZE,
        "max_cube_size": MAX_CUBE_SIZE,
    });
    emit_cc4_evidence(
        "cc4_parsing",
        CPU_REFERENCE_BACKEND,
        CPU_REFERENCE_LANE,
        json!({"section": "10.3.1"}),
        (0, 0),
        json_hash(&metrics),
        metrics,
    );
}

// ---------------------------------------------------------------------------
// §10.3.2: identity.
// ---------------------------------------------------------------------------

/// The §10.2 raster restricted to samples whose every channel lies in `[0, 1]`.
///
/// §10.3.2a's bit-exactness claim is scoped to the in-domain part of the
/// raster, because an out-of-domain sample exercises the §3.5 additive rule
/// rather than the lattice interpolation.
fn in_domain_raster_frame() -> (u32, u32, WorkingFrame) {
    let samples = cc3_parity_raster()
        .into_iter()
        .filter(|rgb| rgb.iter().all(|value| (0.0..=1.0).contains(value)))
        .collect::<Vec<_>>();
    assert!(
        samples.len() >= 100,
        "the in-domain restriction must keep a substantial raster; kept {}",
        samples.len()
    );
    let width = CC3_RASTER_BLOCK_WIDTH * samples.len() as u32;
    let height = CC3_RASTER_HEIGHT;
    let rgb = (0..width * height)
        .map(|index| samples[(index % width / CC3_RASTER_BLOCK_WIDTH) as usize])
        .collect::<Vec<_>>();
    (width, height, working_frame(width, height, &rgb))
}

/// CC4 §10.3.2a. An identity lattice at `S ∈ {2, 17, 33, 65}`, `domain [0, 1]`,
/// `input_encoding = linear`, `mix = 10000` reproduces the in-domain raster
/// **bit-exactly** on the CPU reference and on the GPU.
#[test]
fn cc4_identity_lattices_are_bit_exact_in_linear_on_cpu_and_gpu() {
    let gpu = fallback_gpu();
    let compositor = Compositor::new(gpu.context());
    let (width, height, frame) = in_domain_raster_frame();
    let resolution = (width, height);

    // The f16 working-storage view of the raster: this is the value the node
    // is handed, so it is what "bit-identical to the input" means.
    let input_linear = cpu_reference_linear(&frame, &[]);
    let input_monitor = cpu_reference_monitor(&frame, &[]);
    let gpu_baseline = gpu_linear(&compositor, resolution, &frame, &[], None);
    assert_eq!(
        bits_of(&gpu_baseline),
        bits_of(&input_linear),
        "the look-free GPU path must already reproduce the in-domain raster bit-exactly; without \
         that the identity claim below would be measuring the sampler, not the lattice"
    );

    let mut recorded = Vec::new();
    for size in [2_u32, 17, 33, 65] {
        let lattice = identity_lattice(size);
        let luts = FixtureLuts::one(&format!("cc4-identity-{size}"), &lattice);
        let stack = [creative_look(
            1,
            1,
            LutInputEncoding::Linear.token(),
            10_000,
        )];
        assert!(
            color_node_inactive_reason(&stack[0]).is_none(),
            "the identity case must be an ACTIVE node, or it proves nothing"
        );

        let nodes = cpu_nodes_with(&stack, luts.library());
        assert_eq!(nodes.len(), 1);
        assert_eq!(nodes[0].kind(), ColorNodeKind::CreativeLook);
        let cpu = cpu_reference_linear(&frame, &nodes);
        assert_eq!(
            bits_of(&cpu),
            bits_of(&input_linear),
            "S = {size}: the CPU reference must reproduce the in-domain raster bit-exactly"
        );
        assert_eq!(cpu_reference_monitor(&frame, &nodes), input_monitor);

        let rendered = gpu_linear(
            &compositor,
            resolution,
            &frame,
            &stack,
            Some(luts.library()),
        );
        assert_eq!(
            bits_of(&rendered),
            bits_of(&input_linear),
            "S = {size}: the GPU must reproduce the in-domain raster bit-exactly"
        );
        let rendered_monitor = gpu_monitor(
            &compositor,
            resolution,
            &frame,
            &stack,
            Some(luts.library()),
        );
        assert_eq!(rendered_monitor, input_monitor, "S = {size}: monitor RGBA8");

        recorded.push(json!({
            "size": size,
            "lattice_points": u64::from(size).pow(3),
            "sha256": luts.asset(0).sha256,
            "bit_exact_linear": true,
            "bit_exact_monitor_rgba8": true,
        }));
    }

    let metrics = json!({
        "lane": gpu.lane.id(),
        "in_domain_rgb_samples": input_linear.len() / 4,
        "encoding": LutInputEncoding::Linear.as_str(),
        "mix_basis_points": 10_000,
        "sizes": recorded,
    });
    emit_cc4_evidence(
        "cc4_identity_bit_exact",
        gpu.backend(),
        gpu.lane.id(),
        json!({"section": "10.3.2a", "sizes": [2, 17, 33, 65]}),
        resolution,
        json_hash(&metrics),
        metrics,
    );
}

/// CC4 §10.3.2b. The same identity lattice under `display709` and `grade709`
/// reproduces the *whole* §10.2 raster within `LINEAR_CPU_GPU_MAX`, including
/// the out-of-domain samples the §3.5 additive rule restores, and the CPU and
/// the GPU agree under the §6.2 gates.
#[test]
fn cc4_identity_round_trips_display709_and_grade709_within_the_linear_gate() {
    let gpu = fallback_gpu();
    let compositor = Compositor::new(gpu.context());
    let (width, height, frame) = cc3_raster_frame();
    let resolution = (width, height);
    let input_linear = cpu_reference_linear(&frame, &[]);

    let mut recorded = Vec::new();
    for size in [17_u32, 33] {
        let lattice = identity_lattice(size);
        let luts = FixtureLuts::one(&format!("cc4-identity-encoding-{size}"), &lattice);
        for encoding in [LutInputEncoding::Display709, LutInputEncoding::Grade709] {
            let label = format!("identity_{}_{size}", encoding.as_str());
            let stack = [creative_look(1, 1, encoding.token(), 10_000)];
            // The node must actually be resolved and written, or "the output
            // equals the input" would be a statement about an empty stack.
            let nodes = cpu_nodes_with(&stack, luts.library());
            assert_eq!(nodes.len(), 1, "{label}: the node must be active");
            assert_eq!(nodes[0].kind(), ColorNodeKind::CreativeLook);
            let bytes = grade_buffer_bytes_with_luts(&stack, Some(luts.library()))
                .expect("an active LUT node serializes");
            assert_eq!(grade_header_word(&bytes, 0), 1, "{label}: one active node");
            assert_eq!(
                grade_value(&bytes, 0, 2).to_bits(),
                (encoding.token() as f32).to_bits(),
                "{label}: the encoding token reaches the shader"
            );
            let cpu = cpu_reference_linear(&frame, &nodes);
            let cpu_metrics = linear_parity_metrics(&cpu, &input_linear);
            assert_linear_parity(&cpu_metrics, &format!("cpu_{label}"));

            let rendered = gpu_linear(
                &compositor,
                resolution,
                &frame,
                &stack,
                Some(luts.library()),
            );
            let gpu_metrics = linear_parity_metrics(&rendered, &input_linear);
            assert_linear_parity(&gpu_metrics, &format!("gpu_{label}"));

            // CPU against GPU under the same §6.2 gates.
            let parity = linear_parity_metrics(&rendered, &cpu);
            assert_linear_parity(&parity, &format!("parity_{label}"));
            let monitor = abs_code_diff_rgb(
                &gpu_monitor(
                    &compositor,
                    resolution,
                    &frame,
                    &stack,
                    Some(luts.library()),
                ),
                &cpu_reference_monitor(&frame, &nodes),
            );
            assert!(monitor.max <= MONITOR_CPU_GPU_MAX, "{label}: {monitor:?}");

            recorded.push(json!({
                "size": size,
                "encoding": encoding.as_str(),
                "cpu_vs_input": cpu_metrics.as_json(),
                "gpu_vs_input": gpu_metrics.as_json(),
                "cpu_vs_gpu": parity.as_json(),
                "max_deviation_in_gamut": cpu_metrics.in_gamut.max,
                "max_deviation_over_range": cpu_metrics.over_range.max,
                "monitor_max_code_error": monitor.max,
            }));
        }
    }

    let metrics = json!({
        "lane": gpu.lane.id(),
        "cases": recorded,
        "linear_gate": {
            "in_gamut": {"max": LINEAR_CPU_GPU_MAX, "p99": LINEAR_CPU_GPU_P99, "mean": LINEAR_CPU_GPU_MEAN},
            "over_range": {"max": LINEAR_CPU_GPU_MAX, "p99": LINEAR_OVER_RANGE_P99, "mean": LINEAR_OVER_RANGE_MEAN},
        },
    });
    emit_cc4_evidence(
        "cc4_identity_encodings",
        gpu.backend(),
        gpu.lane.id(),
        json!({"section": "10.3.2b", "encodings": ["display709", "grade709"]}),
        resolution,
        json_hash(&metrics),
        metrics,
    );
}

/// CC4 §10.3.2c. A bypassed non-neutral node, a `mix = 0` node, and an unbound
/// node each produce output bit-identical to the same stack with the node
/// removed, in linear working values and in monitor RGBA8, on CPU and GPU —
/// and the operation layer refuses to store an unbound node at all.
#[test]
fn cc4_inactive_lut_nodes_are_bit_identical_to_the_stack_without_them() {
    let gpu = fallback_gpu();
    let compositor = Compositor::new(gpu.context());
    let (width, height, frame) = cc3_raster_frame();
    let resolution = (width, height);
    let luts = FixtureLuts::one("cc4-inactive", &non_dyadic_look_lattice());

    // The reference stack: a real correction node, so the comparison is not
    // between two empty stacks.
    let baseline = vec![primary_effect(10)];
    let baseline_nodes = cpu_nodes_with(&baseline, luts.library());
    let baseline_linear = cpu_reference_linear(&frame, &baseline_nodes);
    let baseline_monitor = cpu_reference_monitor(&frame, &baseline_nodes);
    let baseline_gpu_linear = gpu_linear(
        &compositor,
        resolution,
        &frame,
        &baseline,
        Some(luts.library()),
    );
    let baseline_gpu_monitor = gpu_monitor(
        &compositor,
        resolution,
        &frame,
        &baseline,
        Some(luts.library()),
    );

    // The same node, ACTIVE, must move the raster or the inactive cases below
    // prove nothing.
    let active = vec![
        primary_effect(10),
        creative_look(1, 1, LutInputEncoding::Display709.token(), 10_000),
    ];
    let active_linear = cpu_reference_linear(&frame, &cpu_nodes_with(&active, luts.library()));
    assert_case_is_not_vacuous(&active_linear, &baseline_linear, "active_look");

    let cases: [(&str, Effect, ColorNodeInactiveReason); 3] = [
        (
            "bypassed",
            with_parameter(
                &creative_look(1, 1, LutInputEncoding::Display709.token(), 10_000),
                "bypass",
                1,
            ),
            ColorNodeInactiveReason::Bypassed,
        ),
        (
            "neutral_mix_zero",
            creative_look(1, 1, LutInputEncoding::Display709.token(), 0),
            ColorNodeInactiveReason::Neutral,
        ),
        (
            // §3.3 makes this unreachable through Core, so it is constructed
            // directly against the resolver, exactly as §10.3.2c requires.
            "unbound",
            creative_look(1, 0, LutInputEncoding::Display709.token(), 10_000),
            ColorNodeInactiveReason::Unbound,
        ),
    ];

    let mut recorded = Vec::new();
    for (label, effect, reason) in cases {
        assert_eq!(
            color_node_inactive_reason(&effect),
            Some(reason),
            "{label}: Core must classify the node inactive on the stored integers"
        );
        let stack = vec![primary_effect(10), effect.clone()];
        let nodes = cpu_nodes_with(&stack, luts.library());
        assert_eq!(
            nodes.len(),
            1,
            "{label}: an inactive node must be skipped by the CPU reference"
        );
        assert_eq!(
            bits_of(&cpu_reference_linear(&frame, &nodes)),
            bits_of(&baseline_linear),
            "{label}: CPU linear must be bit-identical to the node-removed stack"
        );
        assert_eq!(
            cpu_reference_monitor(&frame, &nodes),
            baseline_monitor,
            "{label}: CPU monitor RGBA8 must be bit-identical"
        );

        // An inactive node must not even reach the GPU buffer (§3.6).
        let bytes = grade_buffer_bytes_with_luts(&stack, Some(luts.library()))
            .expect("an inactive LUT node serializes");
        assert_eq!(
            grade_header_word(&bytes, 0),
            1,
            "{label}: only the primary node may be written"
        );
        assert_eq!(
            grade_kind(&bytes, 0),
            ColorNodeKind::Primary.storage_buffer_tag()
        );

        let rendered = gpu_linear(
            &compositor,
            resolution,
            &frame,
            &stack,
            Some(luts.library()),
        );
        assert_eq!(
            bits_of(&rendered),
            bits_of(&baseline_gpu_linear),
            "{label}: GPU linear must be bit-identical to the node-removed stack"
        );
        assert_eq!(
            gpu_monitor(
                &compositor,
                resolution,
                &frame,
                &stack,
                Some(luts.library())
            ),
            baseline_gpu_monitor,
            "{label}: GPU monitor RGBA8 must be bit-identical"
        );
        recorded.push(json!({
            "case": label,
            "inactive_reason": format!("{reason:?}"),
            "bit_identical_linear": true,
            "bit_identical_monitor_rgba8": true,
            "written_to_grade_buffer": false,
        }));
    }

    // §10.3.2c: `AddEffect` and `InsertEffect` refuse an unbound node.
    let mut rejections = Vec::new();
    let base = luts.document();
    for (label, effect) in [
        (
            "lut_asset_id_omitted",
            effect_with(7, "creative_look", &[("mix_basis_points", 10_000)]),
        ),
        (
            "lut_asset_id_zero",
            creative_look(7, 0, LutInputEncoding::Display709.token(), 10_000),
        ),
        (
            "technical_lut_unbound",
            technical_lut(7, 0, LutInputEncoding::Display709.token()),
        ),
    ] {
        for (path, operation) in [
            (
                "add_effect",
                Operation::AddEffect {
                    clip: ClipId(1),
                    effect: effect.clone(),
                },
            ),
            (
                "insert_effect",
                Operation::InsertEffect {
                    clip: ClipId(1),
                    index: 0,
                    effect: effect.clone(),
                },
            ),
        ] {
            let mut document = base.clone();
            let error = operation
                .apply(&mut document)
                .expect_err("an unbound LUT node must be rejected");
            assert_eq!(
                error,
                OpError::MissingLutAsset {
                    clip: ClipId(1),
                    effect: EffectId(7),
                    lut_asset: LutAssetId(0),
                },
                "{label} through {path}"
            );
            assert_eq!(document, base, "a rejection leaves the document untouched");
            rejections.push(json!({"case": label, "path": path, "error": error.to_string()}));
        }
    }

    let metrics = json!({
        "lane": gpu.lane.id(),
        "cases": recorded,
        "unbound_rejections": rejections,
        "raster_rgb_samples": baseline_linear.len() / 4,
    });
    emit_cc4_evidence(
        "cc4_inactive_nodes",
        gpu.backend(),
        gpu.lane.id(),
        json!({"section": "10.3.2c"}),
        resolution,
        json_hash(&metrics),
        metrics,
    );
}

// ---------------------------------------------------------------------------
// §10.3.3 – §10.3.5: anchors, out-of-domain, and mix.
// ---------------------------------------------------------------------------

/// A wide-bar frame holding one block per anchor sample.
fn anchor_frame(samples: &[[f32; 3]]) -> (u32, u32, WorkingFrame) {
    let width = CC3_RASTER_BLOCK_WIDTH * samples.len() as u32;
    let height = CC3_RASTER_HEIGHT;
    let rgb = (0..width * height)
        .map(|index| samples[(index % width / CC3_RASTER_BLOCK_WIDTH) as usize])
        .collect::<Vec<_>>();
    (width, height, working_frame(width, height, &rgb))
}

/// The working frame's own `f16` values, widened, as raw bits.
///
/// This is what "bit-identical to the input" means, expressed without going
/// back through the reference evaluator.
fn frame_working_bits(frame: &WorkingFrame) -> Vec<u32> {
    frame
        .pixels
        .iter()
        .map(|value| value.to_f32().to_bits())
        .collect()
}

/// The RGB triple of block `index` in a frame produced by [`anchor_frame`].
fn block_rgb(values: &[f32], index: usize) -> [f32; 3] {
    let base = (index * CC3_RASTER_BLOCK_WIDTH as usize) * 4;
    [values[base], values[base + 1], values[base + 2]]
}

fn assert_rgb_within(actual: [f32; 3], expected: [f32; 3], tolerance: f32, label: &str) {
    for channel in 0..3 {
        assert!(
            (actual[channel] - expected[channel]).abs() <= tolerance,
            "{label} channel {channel}: actual {} expected {} (tolerance {tolerance})",
            actual[channel],
            expected[channel]
        );
    }
}

/// The narrowed-domain twin of LUT D, `DOMAIN = [0, 0.5]`.
///
/// LUT D's own `[-0.5, 1.5]` domain contains the whole neutral ramp, so a
/// monotonicity fixture built on LUT D alone would never reach the §3.5
/// additive branch. This lattice keeps LUT D's separable values and moves the
/// boundary into the middle of the ramp, so the assertion is about crossing
/// the boundary rather than about staying inside it.
fn lut_e_lattice() -> SpecLattice {
    separable_lattice((0.0, 0.5))
}

/// CC4 §10.3.3. The LUT B, LUT C, and LUT D anchors, hand-derived, exact on
/// the CPU and within the §6.2 linear gate on the GPU; the tetrahedral result
/// is asserted **different** from the trilinear one, and the tie case is
/// asserted equal through all six §3.5 formulas.
#[test]
fn cc4_interpolation_anchors_match_the_hand_derived_values() {
    let gpu = fallback_gpu();
    let compositor = Compositor::new(gpu.context());
    let linear = LutInputEncoding::Linear.token();

    // --- the six §3.5 formulas agree at the tie ------------------------
    // Written out here, term by term, rather than driven by a loop, so the
    // fixture is a transcription of the contract rather than a paraphrase.
    let b = lut_b_lattice();
    let v = |r: u32, g: u32, blue: u32| b.lattice(r, g, blue);
    let (f_r, f_g, f_b) = (0.5_f64, 0.5, 0.5);
    // One line per §3.5 formula, so this reads as a transcription of the
    // contract's branch table rather than as reformatted prose.
    #[rustfmt::skip]
    let tie_formulas: [[f64; 3]; 6] = [
        spec_tetra(v(0,0,0), f_r, v(1,0,0), v(0,0,0), f_g, v(1,1,0), v(1,0,0), f_b, v(1,1,1), v(1,1,0)),
        spec_tetra(v(0,0,0), f_r, v(1,0,0), v(0,0,0), f_g, v(1,1,1), v(1,0,1), f_b, v(1,0,1), v(1,0,0)),
        spec_tetra(v(0,0,0), f_r, v(1,0,1), v(0,0,1), f_g, v(1,1,1), v(1,0,1), f_b, v(0,0,1), v(0,0,0)),
        spec_tetra(v(0,0,0), f_r, v(1,1,1), v(0,1,1), f_g, v(0,1,1), v(0,0,1), f_b, v(0,0,1), v(0,0,0)),
        spec_tetra(v(0,0,0), f_r, v(1,1,1), v(0,1,1), f_g, v(0,1,0), v(0,0,0), f_b, v(0,1,1), v(0,1,0)),
        spec_tetra(v(0,0,0), f_r, v(1,1,0), v(0,1,0), f_g, v(0,1,0), v(0,0,0), f_b, v(1,1,1), v(1,1,0)),
    ];
    for (index, formula) in tie_formulas.iter().enumerate() {
        assert_eq!(
            *formula,
            [0.5, 0.5, 0.5],
            "§3.5 formula {} disagrees at the tie f = (0.5, 0.5, 0.5)",
            index + 1
        );
    }

    // --- the anchors ----------------------------------------------------
    struct Anchor {
        lut: &'static str,
        branch: &'static str,
        input: [f32; 3],
        expected: [f32; 3],
    }
    const ANCHORS: [Anchor; 5] = [
        Anchor {
            lut: "B",
            branch: "f_r > f_g > f_b",
            input: [0.75, 0.50, 0.25],
            expected: [0.500_000, 0.375_000, 0.250_000],
        },
        Anchor {
            lut: "B",
            branch: "f_r <= f_g, f_b > f_g",
            input: [0.25, 0.50, 0.75],
            expected: [0.250_000, 0.375_000, 0.500_000],
        },
        Anchor {
            lut: "B",
            branch: "tie -> final else",
            input: [0.50, 0.50, 0.50],
            expected: [0.500_000, 0.500_000, 0.500_000],
        },
        Anchor {
            lut: "C",
            branch: "separable, in domain",
            input: [0.75, 0.25, 0.50],
            expected: [0.625_000, 0.125_000, 0.250_000],
        },
        Anchor {
            lut: "D",
            branch: "domain mapping",
            input: [0.50, 0.00, 1.00],
            expected: [0.250_000, 0.125_000, 0.625_000],
        },
    ];

    let lattices: [(&str, SpecLattice); 3] = [
        ("B", lut_b_lattice()),
        ("C", lut_c_lattice()),
        ("D", lut_d_lattice()),
    ];
    let luts = FixtureLuts::build(
        "cc4-anchors",
        &lattices
            .iter()
            .map(|(_, lattice)| lattice_cube_text(lattice))
            .collect::<Vec<_>>(),
    );

    let mut recorded = Vec::new();
    for (asset_index, (name, lattice)) in lattices.iter().enumerate() {
        let rows = ANCHORS
            .iter()
            .filter(|anchor| anchor.lut == *name)
            .collect::<Vec<_>>();
        assert!(
            !rows.is_empty(),
            "no anchor row names LUT {name}; a mislabelled row would silently skip every \
             assertion for this lattice"
        );
        let inputs = rows.iter().map(|anchor| anchor.input).collect::<Vec<_>>();
        let (width, height, frame) = anchor_frame(&inputs);
        let stack = [creative_look(1, asset_index as i64 + 1, linear, 10_000)];
        let nodes = cpu_nodes_with(&stack, luts.library());
        let cpu = cpu_reference_linear(&frame, &nodes);
        let rendered = gpu_linear(
            &compositor,
            (width, height),
            &frame,
            &stack,
            Some(luts.library()),
        );

        for (index, anchor) in rows.iter().enumerate() {
            let label = format!("LUT {} {}", anchor.lut, anchor.branch);
            // The independent f64 transcription of §3.5 must agree with the
            // literal the contract states, before either is compared to code.
            let spec = lattice.apply(LutInputEncoding::Linear, 1.0, anchor.input.map(f64::from));
            for channel in 0..3 {
                assert!(
                    (spec[channel] - f64::from(anchor.expected[channel])).abs() <= 1.0e-12,
                    "{label}: the f64 transcription disagrees with the contract literal: {spec:?}"
                );
            }
            // Every anchor value is an exact binary fraction, so the CPU
            // reference is an equality, not a tolerance.
            assert_eq!(
                block_rgb(&cpu, index),
                anchor.expected,
                "{label}: CPU reference must be exact"
            );
            assert_rgb_within(
                block_rgb(&rendered, index),
                anchor.expected,
                ANCHOR_GATE,
                &format!("{label}: GPU"),
            );
            recorded.push(json!({
                "lut": anchor.lut,
                "branch": anchor.branch,
                "input": anchor.input,
                "expected": anchor.expected,
                "cpu": block_rgb(&cpu, index),
                "gpu": block_rgb(&rendered, index),
            }));
        }
    }

    // --- tetrahedral is not trilinear ----------------------------------
    // The contract states the trilinear value of the same lattice at the same
    // input; it is written out here and cross-checked against the
    // test-only eight-vertex evaluator, then asserted to differ from what the
    // production node produced.
    const TRILINEAR_B: [f32; 3] = [0.421_875, 0.296_875, 0.171_875];
    let cube: CubeLut =
        parse_cube_lut_typed(&lattice_cube_text(&lut_b_lattice())).expect("LUT B parses");
    let counter =
        crate::color_pipeline::Lut3d::from_cube(&cube).trilinear_lookup([0.75, 0.5, 0.25]);
    assert_eq!(
        counter, TRILINEAR_B,
        "the eight-vertex counter-implementation must reproduce the contract's trilinear value"
    );
    let (width, height, frame) = anchor_frame(&[[0.75, 0.5, 0.25]]);
    let stack = [creative_look(1, 1, linear, 10_000)];
    let produced = block_rgb(
        &gpu_linear(
            &compositor,
            (width, height),
            &frame,
            &stack,
            Some(luts.library()),
        ),
        0,
    );
    for channel in 0..3 {
        assert!(
            (produced[channel] - TRILINEAR_B[channel]).abs() > ANCHOR_GATE,
            "channel {channel}: the production node produced the TRILINEAR value {}, so \
             tetrahedral interpolation is not actually implemented",
            produced[channel]
        );
    }

    let metrics = json!({
        "lane": gpu.lane.id(),
        "anchors": recorded,
        "trilinear_counter_value": TRILINEAR_B,
        "tetrahedral_value": produced,
        "tie_formulas_agree": true,
        "anchor_gate": ANCHOR_GATE,
    });
    emit_cc4_evidence(
        "cc4_interpolation_anchors",
        gpu.backend(),
        gpu.lane.id(),
        json!({"section": "10.3.3", "luts": ["B", "C", "D"]}),
        (0, 0),
        json_hash(&metrics),
        metrics,
    );
}

/// CC4 §10.3.4. The additive out-of-domain rule restores the excursion on top
/// of the boundary value, the result is asserted **different** from a pure
/// clamp, and monotonicity holds across the boundary on the 8-bit and 10-bit
/// neutral ramps on CPU and GPU.
#[test]
fn cc4_out_of_domain_restores_the_excursion_and_stays_monotone() {
    let gpu = fallback_gpu();
    let compositor = Compositor::new(gpu.context());
    let linear = LutInputEncoding::Linear.token();
    let d = lut_d_lattice();
    let luts = FixtureLuts::build(
        "cc4-out-of-domain",
        &[lattice_cube_text(&d), lattice_cube_text(&lut_e_lattice())],
    );

    // `(2, 2, 2)` clamps to `(1.5, 1.5, 1.5)`, whose lookup is `(1, 1, 1)`, so
    // the node output is `(1, 1, 1) + (2 - 1.5) = (1.5, 1.5, 1.5)`.
    // `(-1, -1, -1)` clamps to `(-0.5, -0.5, -0.5)`, lookup `(0, 0, 0)`,
    // output `(0, 0, 0) + (-1 + 0.5) = (-0.5, -0.5, -0.5)`.
    const CASES: [(&str, [f32; 3], [f32; 3], [f32; 3]); 2] = [
        (
            "above_dmax",
            [2.0, 2.0, 2.0],
            [1.5, 1.5, 1.5],
            [1.0, 1.0, 1.0],
        ),
        (
            "below_dmin",
            [-1.0, -1.0, -1.0],
            [-0.5, -0.5, -0.5],
            [0.0, 0.0, 0.0],
        ),
    ];
    let inputs = CASES.map(|(_, input, _, _)| input);
    let (width, height, frame) = anchor_frame(&inputs);
    let stack = [creative_look(1, 1, linear, 10_000)];
    let nodes = cpu_nodes_with(&stack, luts.library());
    let cpu = cpu_reference_linear(&frame, &nodes);
    let rendered = gpu_linear(
        &compositor,
        (width, height),
        &frame,
        &stack,
        Some(luts.library()),
    );

    let mut recorded = Vec::new();
    for (index, (label, input, expected, pure_clamp)) in CASES.into_iter().enumerate() {
        let spec = d.apply(LutInputEncoding::Linear, 1.0, input.map(f64::from));
        for channel in 0..3 {
            assert!(
                (spec[channel] - f64::from(expected[channel])).abs() <= 1.0e-12,
                "{label}: the f64 transcription disagrees with the contract literal: {spec:?}"
            );
        }
        assert_eq!(
            block_rgb(&cpu, index),
            expected,
            "{label}: the CPU reference must restore the excursion exactly"
        );
        assert_rgb_within(
            block_rgb(&rendered, index),
            expected,
            LINEAR_CPU_GPU_MAX,
            &format!("{label}: GPU"),
        );
        // A pure-clamp implementation would return the boundary lookup value.
        for (channel, clamped) in pure_clamp.into_iter().enumerate() {
            assert!(
                (block_rgb(&cpu, index)[channel] - clamped).abs() > LINEAR_CPU_GPU_MAX,
                "{label} channel {channel}: the CPU reference produced the PURE CLAMP value"
            );
            assert!(
                (block_rgb(&rendered, index)[channel] - clamped).abs() > LINEAR_CPU_GPU_MAX,
                "{label} channel {channel}: the GPU produced the PURE CLAMP value"
            );
        }
        recorded.push(json!({
            "case": label,
            "input": input,
            "expected": expected,
            "pure_clamp_would_be": pure_clamp,
            "cpu": block_rgb(&cpu, index),
            "gpu": block_rgb(&rendered, index),
        }));
    }

    // --- monotonicity across the boundary -------------------------------
    let boundary_stack = [creative_look(1, 2, linear, 10_000)];
    let boundary_nodes = cpu_nodes_with(&boundary_stack, luts.library());
    let mut ramps = Vec::new();
    for depth in [8_u32, 10] {
        let (ramp_width, ramp_height, ramp) = neutral_ramp(depth);
        let crossing = ramp
            .pixels
            .as_chunks::<4>()
            .0
            .iter()
            .filter(|rgba| rgba[0].to_f32() > 0.5)
            .count();
        assert!(
            crossing > 0,
            "{depth}-bit ramp never crosses the narrowed domain boundary, so the case is vacuous"
        );
        let cpu_monitor = cpu_reference_monitor(&ramp, &boundary_nodes);
        let gpu_monitor_codes = gpu_monitor(
            &compositor,
            (ramp_width, ramp_height),
            &ramp,
            &boundary_stack,
            Some(luts.library()),
        );
        let cpu_descending = descending_pairs(&cpu_monitor, ramp_width, ramp_height);
        let gpu_descending = descending_pairs(&gpu_monitor_codes, ramp_width, ramp_height);
        assert_eq!(
            cpu_descending, 0,
            "{depth}-bit ramp: the CPU reference descended across the domain boundary"
        );
        assert_eq!(
            gpu_descending, 0,
            "{depth}-bit ramp: the GPU descended across the domain boundary"
        );
        ramps.push(json!({
            "depth_bits": depth,
            "codes": ramp_width,
            "samples_above_domain_max": crossing,
            "cpu_descending_pairs": cpu_descending,
            "gpu_descending_pairs": gpu_descending,
        }));
    }

    let metrics = json!({
        "lane": gpu.lane.id(),
        "anchors": recorded,
        "boundary_domain": [0.0, 0.5],
        "ramps": ramps,
    });
    emit_cc4_evidence(
        "cc4_out_of_domain",
        gpu.backend(),
        gpu.lane.id(),
        json!({"section": "10.3.4"}),
        (0, 0),
        json_hash(&metrics),
        metrics,
    );
}

/// CC4 §10.3.5. Mix endpoints and midpoint on LUT B, hand-derived, on CPU and
/// GPU; `mix = 0` is bit-identical to removing the node.
#[test]
fn cc4_mix_endpoints_and_midpoint_match_the_hand_derived_values() {
    let gpu = fallback_gpu();
    let compositor = Compositor::new(gpu.context());
    let linear = LutInputEncoding::Linear.token();
    let b = lut_b_lattice();
    let luts = FixtureLuts::one("cc4-mix", &b);

    const INPUT: [f32; 3] = [0.75, 0.50, 0.25];
    // `look(x) = (0.5, 0.375, 0.25)` from §10.3.3, so
    // `out = x + (look(x) - x) * mix`.
    const CASES: [(i64, [f32; 3]); 3] = [
        (0, [0.750_000, 0.500_000, 0.250_000]),
        (5_000, [0.625_000, 0.437_500, 0.250_000]),
        (10_000, [0.500_000, 0.375_000, 0.250_000]),
    ];

    let (width, height, frame) = anchor_frame(&[INPUT]);
    let resolution = (width, height);
    let removed_linear = cpu_reference_linear(&frame, &[]);
    let removed_monitor = cpu_reference_monitor(&frame, &[]);
    let removed_gpu = gpu_linear(&compositor, resolution, &frame, &[], None);
    let removed_gpu_monitor = gpu_monitor(&compositor, resolution, &frame, &[], None);

    let mut recorded = Vec::new();
    for (mix, expected) in CASES {
        let stack = [creative_look(1, 1, linear, mix)];
        let spec = b.apply(
            LutInputEncoding::Linear,
            f64::from(mix as f32) / 10_000.0,
            INPUT.map(f64::from),
        );
        for channel in 0..3 {
            assert!(
                (spec[channel] - f64::from(expected[channel])).abs() <= 1.0e-12,
                "mix {mix}: the f64 transcription disagrees with the contract literal: {spec:?}"
            );
        }
        let nodes = cpu_nodes_with(&stack, luts.library());
        let cpu = cpu_reference_linear(&frame, &nodes);
        assert_eq!(block_rgb(&cpu, 0), expected, "mix {mix}: CPU reference");
        let rendered = gpu_linear(
            &compositor,
            resolution,
            &frame,
            &stack,
            Some(luts.library()),
        );
        assert_rgb_within(
            block_rgb(&rendered, 0),
            expected,
            LINEAR_CPU_GPU_MAX,
            &format!("mix {mix}: GPU"),
        );

        if mix == 0 {
            // §3.6: `mix = 0` is decided on the stored integer, so the node is
            // never written and the result is bit-identical to removal.
            assert_eq!(
                color_node_inactive_reason(&stack[0]),
                Some(ColorNodeInactiveReason::Neutral)
            );
            assert_eq!(nodes.len(), 0);
            // Compared against the frame's own working values, not against
            // `cpu_reference_linear(frame, &[])`, which would be the same
            // function with the same arguments on both sides.
            assert_eq!(
                bits_of(&cpu),
                frame_working_bits(&frame),
                "mix = 0 must leave the working values bit-identical to the input"
            );
            assert_eq!(
                removed_linear, cpu,
                "the node-removed stack is the same picture"
            );
            assert_eq!(cpu_reference_monitor(&frame, &nodes), removed_monitor);
            assert_eq!(bits_of(&rendered), bits_of(&removed_gpu));
            assert_eq!(
                gpu_monitor(
                    &compositor,
                    resolution,
                    &frame,
                    &stack,
                    Some(luts.library())
                ),
                removed_gpu_monitor
            );
        }
        recorded.push(json!({
            "mix_basis_points": mix,
            "expected": expected,
            "cpu": block_rgb(&cpu, 0),
            "gpu": block_rgb(&rendered, 0),
            "bit_identical_to_removal": mix == 0,
        }));
    }

    let metrics = json!({
        "lane": gpu.lane.id(),
        "input": INPUT,
        "look_of_input": [0.5, 0.375, 0.25],
        "cases": recorded,
    });
    emit_cc4_evidence(
        "cc4_mix",
        gpu.backend(),
        gpu.lane.id(),
        json!({"section": "10.3.5"}),
        resolution,
        json_hash(&metrics),
        metrics,
    );
}

// ---------------------------------------------------------------------------
// §10.3.6: stage ordering.
// ---------------------------------------------------------------------------

/// The technical input transform used by the ordering and parity fixtures: a
/// 17³ channel rotation with a slight gain.
///
/// It does **not** commute with the creative look, which is what makes the
/// ordering claim measurable rather than decorative.
fn technical_lattice() -> SpecLattice {
    SpecLattice::new(17, (0.0, 1.0), |e| [e[1] * 0.94, e[2] * 0.97, e[0] * 0.91])
}

/// The full CC4 §3.1 stack, in stage order, bound to the two fixture lattices.
fn five_kind_stack() -> Vec<Effect> {
    vec![
        technical_lut(1, 1, LutInputEncoding::Display709.token()),
        primary_effect(2),
        representative_wheels(3),
        representative_curves(4),
        creative_look(5, 2, LutInputEncoding::Display709.token(), 7_500),
    ]
}

/// CC4 §10.3.6. The serialized vector order is the execution order, the
/// reversed order produces a different result, storing it is rejected with a
/// fully specified `ColorStageOrderViolation` through all three paths, and a
/// legal `InsertEffect` preserves every other effect's relative order.
#[test]
fn cc4_stage_order_is_the_execution_order_and_a_violation_is_rejected() {
    let gpu = fallback_gpu();
    let compositor = Compositor::new(gpu.context());
    let (width, height, frame) = cc3_raster_frame();
    let resolution = (width, height);
    let luts = FixtureLuts::build(
        "cc4-ordering",
        &[
            lattice_cube_text(&technical_lattice()),
            lattice_cube_text(&non_dyadic_look_lattice()),
        ],
    );
    let baseline = cpu_reference_linear(&frame, &[]);

    // --- the legal five-kind stack -------------------------------------
    let stack = five_kind_stack();
    let nodes = cpu_nodes_with(&stack, luts.library());
    assert_eq!(
        nodes.iter().map(ColorNode::kind).collect::<Vec<_>>(),
        vec![
            ColorNodeKind::TechnicalLut,
            ColorNodeKind::Primary,
            ColorNodeKind::Wheels,
            ColorNodeKind::Curves,
            ColorNodeKind::CreativeLook,
        ],
        "the resolved stack must follow clip.effects vector order"
    );
    assert_eq!(
        active_color_nodes(&stack)
            .into_iter()
            .map(|(index, kind)| (index, kind.stage().rank()))
            .collect::<Vec<_>>(),
        vec![(0, 0), (1, 1), (2, 1), (3, 1), (4, 2)],
        "stage ranks must be non-decreasing in the serialized order"
    );
    let (monitor, linear, _) = assert_gpu_case_with_luts(
        &compositor,
        resolution,
        &frame,
        &stack,
        luts.library(),
        Some(&baseline),
        "five_kind_stage_order",
    );

    // --- the reversed order is a different picture ----------------------
    // The document cannot store this order, so it is evaluated directly
    // against the CPU reference, exactly as §10.3.6 requires.
    let forward_pair = vec![
        technical_lut(1, 1, LutInputEncoding::Display709.token()),
        creative_look(5, 2, LutInputEncoding::Display709.token(), 7_500),
    ];
    let reversed_pair = vec![forward_pair[1].clone(), forward_pair[0].clone()];
    let forward_linear =
        cpu_reference_linear(&frame, &cpu_nodes_with(&forward_pair, luts.library()));
    let reversed_linear =
        cpu_reference_linear(&frame, &cpu_nodes_with(&reversed_pair, luts.library()));
    let mut worst = 0.0_f32;
    let mut differing = 0_usize;
    for (a, b) in forward_linear
        .as_chunks::<4>()
        .0
        .iter()
        .zip(reversed_linear.as_chunks::<4>().0.iter())
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
        "the two stage orders must differ by more than the parity gate: max difference {worst}"
    );
    assert!(
        differing * 10_000
            >= (forward_linear.len() / 4 * 3) * MIN_CHANGED_LINEAR_BASIS_POINTS as usize,
        "only {differing} samples differ between the two stage orders"
    );

    // --- the violation is rejected, not reordered -----------------------
    let expected = OpError::ColorStageOrderViolation {
        clip: ClipId(1),
        effect: EffectId(1),
        kind: "technical_lut".to_owned(),
        color_stage_rank: ColorStage::Input.rank(),
        previous_effect: EffectId(5),
        previous_kind: "creative_look".to_owned(),
        previous_color_stage_rank: ColorStage::Look.rank(),
    };
    let mut with_look = luts.document();
    Operation::AddEffect {
        clip: ClipId(1),
        effect: forward_pair[1].clone(),
    }
    .apply(&mut with_look)
    .expect("a bound creative look is legal on its own");

    let mut rejections = Vec::new();
    for (path, operation) in [
        (
            "add_effect",
            Operation::AddEffect {
                clip: ClipId(1),
                effect: forward_pair[0].clone(),
            },
        ),
        (
            "insert_effect",
            Operation::InsertEffect {
                clip: ClipId(1),
                index: 1,
                effect: forward_pair[0].clone(),
            },
        ),
    ] {
        let mut document = with_look.clone();
        let error = operation
            .apply(&mut document)
            .expect_err("a technical LUT after a creative look must be rejected");
        assert_eq!(error, expected, "{path}");
        assert_eq!(
            stage_ranks(&error),
            (ColorStage::Input.rank(), ColorStage::Look.rank()),
            "{path}: the returned error must name the offending and the previous stage rank"
        );
        assert_eq!(
            document, with_look,
            "a rejection leaves the document untouched"
        );
        rejections.push(json!({"path": path, "error": error.to_string()}));
    }

    // `validate_document` is Core-internal, so it is reached the only way a
    // caller can: every operation validates the *incoming* document before it
    // touches it, so a hand-built violating document rejects any edit at all.
    let mut violating = luts.document();
    violating.tracks[0].clips[0].effects = reversed_pair.clone();
    let error = Operation::SetTitleParam {
        clip: ClipId(1),
        name: "text".to_owned(),
        value: ParamValue::Text("unrelated".to_owned()),
    }
    .apply(&mut violating.clone())
    .expect_err("a document that already violates the stage order rejects every edit");
    assert_eq!(error, expected, "validate_document");
    assert_eq!(
        stage_ranks(&error),
        (ColorStage::Input.rank(), ColorStage::Look.rank()),
        "validate_document must name both stage ranks too"
    );
    rejections.push(json!({"path": "validate_document", "error": error.to_string()}));

    // --- a legal insertion preserves relative order ---------------------
    let mut ordered = luts.document();
    for effect in [
        primary_effect(2),
        representative_wheels(3),
        forward_pair[1].clone(),
    ] {
        Operation::AddEffect {
            clip: ClipId(1),
            effect,
        }
        .apply(&mut ordered)
        .expect("the correction and look nodes are legal in this order");
    }
    let before = ordered.tracks[0].clips[0]
        .effects
        .iter()
        .map(|effect| effect.id.0)
        .collect::<Vec<_>>();
    assert_eq!(before, vec![2, 3, 5]);
    Operation::InsertEffect {
        clip: ClipId(1),
        index: 0,
        effect: forward_pair[0].clone(),
    }
    .apply(&mut ordered)
    .expect("a technical LUT ahead of the corrections is legal");
    let after = ordered.tracks[0].clips[0]
        .effects
        .iter()
        .map(|effect| effect.id.0)
        .collect::<Vec<_>>();
    assert_eq!(
        after,
        vec![1, 2, 3, 5],
        "InsertEffect must place the node at the index"
    );
    assert_eq!(
        after
            .iter()
            .copied()
            .filter(|id| *id != 1)
            .collect::<Vec<_>>(),
        before,
        "every other effect keeps its relative order"
    );

    let metrics = json!({
        "lane": gpu.lane.id(),
        "stack": stack.iter().map(|effect| effect.name.as_str()).collect::<Vec<_>>(),
        "stage_ranks": [0, 1, 1, 1, 2],
        "order_max_linear_difference": worst,
        "order_differing_rgb_samples": differing,
        "monitor_max_code_error": monitor.max,
        "monitor_p99_code_error": monitor.p99,
        "monitor_mean_code_error": monitor.mean,
        "linear": linear.as_json(),
        "rejections": rejections,
        "insert_effect_order_before": before,
        "insert_effect_order_after": after,
    });
    emit_cc4_evidence(
        "cc4_node_ordering",
        gpu.backend(),
        gpu.lane.id(),
        json!({"section": "10.3.6"}),
        resolution,
        json_hash(&metrics),
        metrics,
    );
}

/// The two stage ranks the *returned* `ColorStageOrderViolation` carries, so
/// each rejection path is asserted to name them rather than only being
/// compared as one whole struct.
fn stage_ranks(error: &OpError) -> (u8, u8) {
    match error {
        OpError::ColorStageOrderViolation {
            color_stage_rank,
            previous_color_stage_rank,
            ..
        } => (*color_stage_rank, *previous_color_stage_rank),
        other => panic!("expected a stage-order violation, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// §10.2 raster coverage and §10.3.7 CPU/GPU parity.
// ---------------------------------------------------------------------------

/// How many of the 192 §10.2 raster samples encode outside `[0, 1]` in
/// `display709`, and therefore exercise the §3.5 additive out-of-domain rule.
fn raster_samples_outside_unit_display() -> usize {
    cc3_parity_raster()
        .into_iter()
        .filter(|rgb| {
            rgb.iter().any(|value| {
                let encoded = spec_encode_bt709_signed_f64(f64::from(*value));
                !(0.0..=1.0).contains(&encoded)
            })
        })
        .count()
}

/// CC4 §10.2. The reused CC3 raster asserts its own out-of-domain coverage, so
/// a LUT parity run can never be quietly in-domain everywhere.
#[test]
fn cc4_parity_raster_exercises_the_out_of_domain_rule() {
    let samples = cc3_parity_raster();
    assert_eq!(
        samples.len(),
        192,
        "CC4 §10.2 reuses the CC3 raster verbatim"
    );
    assert_eq!(CC3_RASTER_LEVELS.len() * CC3_PATTERNS.len(), 192);
    let outside = raster_samples_outside_unit_display();
    assert!(
        outside >= MIN_OUT_OF_DOMAIN_RASTER_SAMPLES,
        "only {outside} of 192 raster samples encode outside [0, 1]; CC4 §10.2 requires at least \
         {MIN_OUT_OF_DOMAIN_RASTER_SAMPLES}"
    );
    assert_eq!(
        outside, 72,
        "the contract records 72 out-of-domain samples; a change here is a raster change"
    );
}

/// The §10.3.7 parity body, shared by the software and hardware lanes.
fn assert_cc4_gpu_parity(gpu: &FixtureGpu) {
    // §10.1.4: the gate constants are the CC1 §6.2 numbers, asserted against
    // the code rather than restated.
    assert_eq!(MONITOR_CPU_GPU_MAX, 2);
    assert_eq!(MONITOR_CPU_GPU_P99, 1.0);
    assert_eq!(MONITOR_CPU_GPU_MEAN, 0.50);
    assert_eq!(LINEAR_CPU_GPU_MAX, 1.5e-3);
    assert_eq!(LINEAR_CPU_GPU_P99, 7.5e-4);
    assert_eq!(LINEAR_CPU_GPU_MEAN, 2.5e-4);
    assert_eq!(LINEAR_OVER_RANGE_P99, 9.765_625e-4);
    assert_eq!(LINEAR_OVER_RANGE_MEAN, 9.765_625e-4);
    assert_eq!(LINEAR_GATE_IN_GAMUT, 1.0);
    assert_eq!(LINEAR_GATE_DOMAIN, 2.0);

    let compositor = Compositor::new(gpu.context());
    let (width, height, frame) = cc3_raster_frame();
    let resolution = (width, height);
    let outside = raster_samples_outside_unit_display();
    assert!(outside >= MIN_OUT_OF_DOMAIN_RASTER_SAMPLES);

    let luts = FixtureLuts::build(
        "cc4-parity",
        &[
            lattice_cube_text(&technical_lattice()),
            lattice_cube_text(&non_dyadic_look_lattice()),
        ],
    );
    // §10.1 rule 7: the precision gate uses a real non-dyadic 33³ look. A
    // lattice sample that is exactly representable in f16 would make the
    // lattice-precision claim vacuous, so the fixture proves it is not.
    let look_cube = luts
        .library()
        .get(LutAssetId(2))
        .expect("the parity look is verified");
    let non_dyadic = (0..look_cube.sample_count() / 3)
        .filter_map(|index| look_cube.sample(index))
        .flatten()
        .filter(|value| f16::from_f32(*value).to_f32() != *value)
        .count();
    assert!(
        non_dyadic * 2 > look_cube.sample_count(),
        "the parity look must be genuinely non-dyadic; only {non_dyadic} of {} samples change \
         under f16 rounding",
        look_cube.sample_count()
    );

    let baseline = cpu_reference_linear(&frame, &[]);
    let baseline_gpu = gpu_linear(&compositor, resolution, &frame, &[], None);

    let mut cases: Vec<(String, Vec<Effect>, bool)> = Vec::new();
    for mix in [0_i64, 5_000, 10_000] {
        cases.push((
            format!("non_dyadic_look_mix_{mix}"),
            vec![creative_look(
                5,
                2,
                LutInputEncoding::Display709.token(),
                mix,
            )],
            mix != 0,
        ));
    }
    cases.push(("five_kind_stack".to_owned(), five_kind_stack(), true));

    let mut recorded = Vec::new();
    for (label, stack, non_neutral) in &cases {
        if *non_neutral {
            let (monitor, linear, _) = assert_gpu_case_with_luts(
                &compositor,
                resolution,
                &frame,
                stack,
                luts.library(),
                Some(&baseline),
                label,
            );
            assert_eq!(linear.non_finite, 0);
            recorded.push(json!({
                "case": label,
                "nodes": stack.iter().map(|effect| effect.name.as_str()).collect::<Vec<_>>(),
                "monitor_max_code_error": monitor.max,
                "monitor_p99_code_error": monitor.p99,
                "monitor_mean_code_error": monitor.mean,
                "linear": linear.as_json(),
                "above_domain_excluded": linear.above_domain,
                "non_finite": linear.non_finite,
                "vacuity_checked": true,
            }));
        } else {
            // `mix = 0` is the neutral endpoint: it must be *bit-identical* to
            // the look-free stack, so the vacuity gate does not apply to it.
            let nodes = cpu_nodes_with(stack, luts.library());
            assert!(nodes.is_empty());
            let rendered = gpu_linear(&compositor, resolution, &frame, stack, Some(luts.library()));
            assert_eq!(bits_of(&rendered), bits_of(&baseline_gpu));
            let linear = linear_parity_metrics(&rendered, &baseline);
            assert_linear_parity(&linear, label);
            recorded.push(json!({
                "case": label,
                "nodes": stack.iter().map(|effect| effect.name.as_str()).collect::<Vec<_>>(),
                "bit_identical_to_look_free": true,
                "linear": linear.as_json(),
                "above_domain_excluded": linear.above_domain,
                "non_finite": linear.non_finite,
            }));
        }
    }

    let metrics = json!({
        "lane": gpu.lane.id(),
        "raster_rgb_samples": 192,
        "raster_samples_outside_unit_display": outside,
        "look": {
            "size": NON_DYADIC_LOOK_SIZE,
            "sha256": luts.asset(1).sha256,
            "non_dyadic_scalar_samples": non_dyadic,
            "scalar_samples": look_cube.sample_count(),
        },
        "cases": recorded,
        "linear_gate": {
            "in_gamut": {"max": LINEAR_CPU_GPU_MAX, "p99": LINEAR_CPU_GPU_P99, "mean": LINEAR_CPU_GPU_MEAN},
            "over_range": {"max": LINEAR_CPU_GPU_MAX, "p99": LINEAR_OVER_RANGE_P99, "mean": LINEAR_OVER_RANGE_MEAN},
            "in_gamut_band": LINEAR_GATE_IN_GAMUT,
            "domain_band": LINEAR_GATE_DOMAIN,
        },
        "monitor_gate": {"max": MONITOR_CPU_GPU_MAX, "p99": MONITOR_CPU_GPU_P99, "mean": MONITOR_CPU_GPU_MEAN},
        "minimum_changed_linear_basis_points": MIN_CHANGED_LINEAR_BASIS_POINTS,
    });
    emit_cc4_evidence(
        "cc4_gpu_cpu_parity",
        gpu.backend(),
        gpu.lane.id(),
        json!({"section": "10.3.7", "cases": cases.iter().map(|(label, _, _)| label.clone()).collect::<Vec<_>>()}),
        resolution,
        json_hash(&metrics),
        metrics,
    );
}

/// CC4 §10.3.7, default lane: the software fallback (lavapipe/llvmpipe/WARP).
#[test]
fn cc4_gpu_compositor_matches_the_cpu_reference_on_software_fallback() {
    assert_cc4_gpu_parity(&fallback_gpu());
}

/// CC4 §10.3.7, explicit hardware lane.
#[test]
#[ignore = "requires a physical supported GPU; run explicitly with --ignored --nocapture"]
fn cc4_gpu_compositor_matches_the_cpu_reference_on_hardware() {
    assert_cc4_gpu_parity(&hardware_gpu());
}

// ---------------------------------------------------------------------------
// §10.3.8: slots, limits, and the ABI.
// ---------------------------------------------------------------------------

/// The four mixed-size lattices §10.3.8 names, each a different affine map so
/// any slot confusion changes the composed result.
fn mixed_size_lattices() -> [SpecLattice; 4] {
    [
        SpecLattice::new(2, (0.0, 1.0), |e| [e[0] * 0.5, e[1] * 0.5, e[2] * 0.5]),
        SpecLattice::new(17, (0.0, 1.0), |e| [e[0], e[2], e[1]]),
        SpecLattice::new(33, (0.0, 1.0), |e| [e[0] * 4.0, e[1], e[2]]),
        SpecLattice::new(65, (0.0, 1.0), |e| [e[0], e[1], e[2] * 0.5]),
    ]
}

/// CC4 §10.3.8. Four LUT nodes take four distinct atlas slots at four distinct
/// `z_origin`s and compose in vector order; a fifth is rejected; the atlas and
/// ABI constants are asserted against the code and the shader.
#[test]
fn cc4_lut_slots_limits_and_abi_constants_hold() {
    let gpu = fallback_gpu();
    let compositor = Compositor::new(gpu.context());
    let linear = LutInputEncoding::Linear.token();

    // --- the constants --------------------------------------------------
    assert_eq!(COMPOSITOR_LUT_SLOTS_PER_LAYER, 4);
    assert_eq!(COMPOSITOR_LUT_SLOTS_PER_LAYER, LUT_NODE_LIMIT_PER_LAYER);
    assert_eq!(COMPOSITOR_LEGACY_LUT_SLOT, 4);
    assert_eq!(COMPOSITOR_LUT_ATLAS_SLOTS, 5);
    // CC5 §3.1 widens the binding to hold sixteen curve-plus-matte nodes;
    // the binding *count* is unchanged, which is the portability claim.
    assert_eq!(COMPOSITOR_REQUIRED_STORAGE_BUFFER_BINDING_SIZE, 32_768);
    assert_eq!(COMPOSITOR_REQUIRED_STORAGE_BUFFERS_PER_SHADER_STAGE, 1);
    assert_eq!(COMPOSITOR_REQUIRED_TEXTURE_DIMENSION_3D, 512);
    assert!(
        COMPOSITOR_REQUIRED_TEXTURE_DIMENSION_3D as usize
            >= COMPOSITOR_LUT_ATLAS_SLOTS * MAX_CUBE_SIZE as usize,
        "the negotiated 3D dimension must hold the worst-case depth-packed atlas"
    );
    assert_eq!(COMPOSITOR_LUT_ATLAS_SLOTS * MAX_CUBE_SIZE as usize, 325);

    // --- every ColorNodeKind has a shader branch ------------------------
    let shader = include_str!("compositor.wgsl");
    for kind in [
        ColorNodeKind::TechnicalLut,
        ColorNodeKind::Primary,
        ColorNodeKind::Wheels,
        ColorNodeKind::Curves,
        ColorNodeKind::CreativeLook,
    ] {
        let branch = format!("kind == {}u", kind.storage_buffer_tag());
        assert!(
            shader.contains(&branch),
            "compositor.wgsl has no dispatch branch for {} ({branch})",
            kind.effect_name()
        );
    }
    assert!(
        shader.contains("textureLoad(\n        lut_texture,")
            || shader.contains("textureLoad(lut_texture"),
        "the atlas at binding 3 must be read with textureLoad (CC4 §4.3)"
    );
    assert!(
        !shader.contains("textureSample(lut_texture"),
        "hardware filtering of the LUT atlas is forbidden by CC4 §4.3; the sampler at binding 1 \
         is never used for the atlas"
    );

    // --- four nodes, four slots, four z_origins -------------------------
    let lattices = mixed_size_lattices();
    let luts = FixtureLuts::build(
        "cc4-slots",
        &lattices.iter().map(lattice_cube_text).collect::<Vec<_>>(),
    );
    let stack = (0..4)
        .map(|index| creative_look(index as u64 + 1, index + 1, linear, 10_000))
        .collect::<Vec<_>>();

    let bytes = grade_buffer_bytes_with_luts(&stack, Some(luts.library()))
        .expect("four LUT nodes serialize");
    assert_eq!(grade_header_word(&bytes, 0), 4, "four active nodes");
    assert_eq!(
        grade_header_word(&bytes, 2),
        3,
        "CC4 §4.2 took GRADE_ABI_VERSION from 1 to 2; CC5 §3.1 takes it to 3, because a \
         consumer that understands only the CC4 kinds would read v11 as a reserved zero"
    );
    const EXPECTED_SLOTS: [(f32, f32, f32); 4] = [
        (0.0, 2.0, 0.0),
        (1.0, 17.0, 2.0),
        (2.0, 33.0, 19.0),
        (3.0, 65.0, 52.0),
    ];
    let mut slot_rows = Vec::new();
    for (node, (slot, size, z_origin)) in EXPECTED_SLOTS.into_iter().enumerate() {
        assert_eq!(
            grade_kind(&bytes, node),
            ColorNodeKind::CreativeLook.storage_buffer_tag()
        );
        assert_eq!(
            grade_value(&bytes, node, 0).to_bits(),
            slot.to_bits(),
            "slot {node}"
        );
        assert_eq!(
            grade_value(&bytes, node, 9).to_bits(),
            size.to_bits(),
            "size {node}"
        );
        assert_eq!(
            grade_value(&bytes, node, 10).to_bits(),
            z_origin.to_bits(),
            "z_origin {node}"
        );
        assert_eq!(grade_value(&bytes, node, 11), 0.0, "v11 is reserved");
        slot_rows.push(json!({"slot": slot, "size": size, "z_origin": z_origin}));
    }
    // Everything below is read back out of the buffer the shader consumes,
    // never out of the literal table above, so the distinctness and extent
    // claims are statements about production values.
    let produced: Vec<(u32, u32, u32)> = (0..4)
        .map(|node| {
            (
                grade_value(&bytes, node, 0) as u32,
                grade_value(&bytes, node, 9) as u32,
                grade_value(&bytes, node, 10) as u32,
            )
        })
        .collect();
    for index in 1..produced.len() {
        assert!(
            produced[index].0 > produced[index - 1].0,
            "atlas slots must be distinct and ascending: {produced:?}"
        );
        assert!(
            produced[index].2 > produced[index - 1].2,
            "atlas z_origins must be distinct and ascending: {produced:?}"
        );
    }
    // The atlas is `(Smax, Smax, sum of the bound slot sizes)`: the depth is
    // the last slot's origin plus its size and `Smax` is the largest bound
    // edge, both taken from the production slot records.
    let depth = produced
        .iter()
        .map(|(_, size, z_origin)| z_origin + size)
        .max()
        .expect("four bound slots");
    let smax = produced
        .iter()
        .map(|(_, size, _)| *size)
        .max()
        .expect("four bound slots");
    assert_eq!(
        (smax, smax, depth),
        (65, 65, 117),
        "CC4 §10.3.8's mixed-size atlas extent, derived from the production slot records \
         (2 + 17 + 33 + 65 = 117)"
    );
    assert_eq!(
        depth,
        produced.iter().map(|(_, size, _)| size).sum::<u32>(),
        "only bound slots are allocated, so the depth is the sum of their sizes"
    );
    // The binding itself must build on this adapter, or the layout above is
    // describing an atlas nothing allocated.
    compositor
        .lut_binding(&stack, Some(luts.library()))
        .expect("four bound LUT nodes fit the atlas");

    // --- and the composition is correct ---------------------------------
    // (0.25, 0.75, 0.5)
    //   x0.5      -> (0.125, 0.375, 0.25)
    //   swap g,b  -> (0.125, 0.25,  0.375)
    //   red x4    -> (0.5,   0.25,  0.375)
    //   blue x0.5 -> (0.5,   0.25,  0.1875)
    const INPUT: [f32; 3] = [0.25, 0.75, 0.5];
    const EXPECTED: [f32; 3] = [0.5, 0.25, 0.187_5];
    let (width, height, frame) = anchor_frame(&[INPUT]);
    let mut composed = INPUT.map(f64::from);
    for lattice in &lattices {
        composed =
            lattice
                .quantized_like_cube_text()
                .apply(LutInputEncoding::Linear, 1.0, composed);
    }
    for channel in 0..3 {
        assert!(
            (composed[channel] - f64::from(EXPECTED[channel])).abs() <= 1.0e-6,
            "the f64 transcription disagrees with the hand-composed value: {composed:?}"
        );
    }
    let rendered = gpu_linear(
        &compositor,
        (width, height),
        &frame,
        &stack,
        Some(luts.library()),
    );
    assert_rgb_within(
        block_rgb(&rendered, 0),
        EXPECTED,
        LINEAR_CPU_GPU_MAX,
        "four_slots",
    );
    let cpu = cpu_reference_linear(&frame, &cpu_nodes_with(&stack, luts.library()));
    assert_rgb_within(
        block_rgb(&cpu, 0),
        EXPECTED,
        LINEAR_CPU_GPU_MAX,
        "four_slots_cpu",
    );
    // A shader that read slot 0 four times would produce this instead.
    let all_slot_zero = INPUT.map(|value| value * 0.0625);
    for (channel, wrong) in all_slot_zero.into_iter().enumerate() {
        assert!(
            (block_rgb(&rendered, 0)[channel] - wrong).abs() > LINEAR_CPU_GPU_MAX,
            "the stack collapsed onto a single atlas slot"
        );
    }

    // --- a fifth LUT node is rejected -----------------------------------
    let mut document = luts.document();
    for effect in &stack {
        Operation::AddEffect {
            clip: ClipId(1),
            effect: effect.clone(),
        }
        .apply(&mut document)
        .expect("four LUT nodes are legal");
    }
    let fifth = creative_look(5, 1, linear, 10_000);
    let error = Operation::AddEffect {
        clip: ClipId(1),
        effect: fifth,
    }
    .apply(&mut document.clone())
    .expect_err("a fifth LUT node must be rejected");
    assert_eq!(
        error,
        OpError::TooManyLutNodes {
            clip: ClipId(1),
            limit: 4,
            actual: 5,
        }
    );

    let metrics = json!({
        "lane": gpu.lane.id(),
        "slots": slot_rows,
        "atlas_extent": [65, 65, depth],
        "grade_abi_version": grade_header_word(&bytes, 2),
        "constants": {
            "COMPOSITOR_LUT_SLOTS_PER_LAYER": COMPOSITOR_LUT_SLOTS_PER_LAYER,
            "COMPOSITOR_LEGACY_LUT_SLOT": COMPOSITOR_LEGACY_LUT_SLOT,
            "COMPOSITOR_LUT_ATLAS_SLOTS": COMPOSITOR_LUT_ATLAS_SLOTS,
            "COMPOSITOR_REQUIRED_STORAGE_BUFFER_BINDING_SIZE": COMPOSITOR_REQUIRED_STORAGE_BUFFER_BINDING_SIZE,
            "COMPOSITOR_REQUIRED_STORAGE_BUFFERS_PER_SHADER_STAGE": COMPOSITOR_REQUIRED_STORAGE_BUFFERS_PER_SHADER_STAGE,
            "COMPOSITOR_REQUIRED_TEXTURE_DIMENSION_3D": COMPOSITOR_REQUIRED_TEXTURE_DIMENSION_3D,
            "LUT_NODE_LIMIT_PER_LAYER": LUT_NODE_LIMIT_PER_LAYER,
            "MAX_CUBE_SIZE": MAX_CUBE_SIZE,
        },
        "composed_input": INPUT,
        "composed_expected": EXPECTED,
        "composed_gpu": block_rgb(&rendered, 0),
        "fifth_node_error": error.to_string(),
        "shader_branches": [1, 2, 3, 4, 5],
    });
    emit_cc4_evidence(
        "cc4_slots_and_limits",
        gpu.backend(),
        gpu.lane.id(),
        json!({"section": "10.3.8"}),
        (width, height),
        json_hash(&metrics),
        metrics,
    );
}

// ---------------------------------------------------------------------------
// §10.3.9: legacy coexistence.
// ---------------------------------------------------------------------------

/// CC4 §10.3.9. A managed `creative_look` and a legacy `cube_lut` coexist; the
/// legacy stage runs after every managed node **regardless of their relative
/// order** in `clip.effects`, and `qa_document` reports `legacy_lut_stage`
/// exactly once in each ordering.
#[test]
fn cc4_legacy_cube_lut_runs_last_beside_a_managed_look() {
    let gpu = fallback_gpu();
    let compositor = Compositor::new(gpu.context());
    let linear = LutInputEncoding::Linear.token();
    let luts = FixtureLuts::one("cc4-legacy", &non_dyadic_look_lattice());

    // A real external `.cube` on disk, loaded through the legacy path.
    let legacy_path = luts.directory().path("legacy.cube");
    fs::write(
        &legacy_path,
        lattice_cube_text(&SpecLattice::new(17, (0.0, 1.0), |e| {
            [e[0] * 0.8, e[1] * 0.9, e[2]]
        })),
    )
    .expect("the legacy .cube is written");
    let legacy = Effect {
        id: EffectId(9),
        name: "cube_lut".to_owned(),
        parameters: BTreeMap::from([
            (
                "path".to_owned(),
                ParamValue::Text(legacy_path.to_string_lossy().into_owned()),
            ),
            ("intensity_percent".to_owned(), ParamValue::Integer(100)),
        ]),
        keyframes: BTreeMap::new(),
    };
    let look = creative_look(1, 1, linear, 10_000);

    let (width, height, frame) = cc3_raster_frame();
    let resolution = (width, height);
    let managed_only = gpu_monitor(
        &compositor,
        resolution,
        &frame,
        std::slice::from_ref(&look),
        Some(luts.library()),
    );

    let orders: [(&str, Vec<Effect>); 2] = [
        ("look_then_legacy", vec![look.clone(), legacy.clone()]),
        ("legacy_then_look", vec![legacy.clone(), look.clone()]),
    ];
    let mut rendered = Vec::new();
    let mut recorded = Vec::new();
    for (label, stack) in &orders {
        // The legacy stage is not a managed node and never joins the stage
        // ordering, so both vector orders are storable.
        let mut document = luts.document();
        for effect in stack {
            Operation::AddEffect {
                clip: ClipId(1),
                effect: effect.clone(),
            }
            .apply(&mut document)
            .unwrap_or_else(|error| panic!("{label} must be storable: {error}"));
        }
        assert_eq!(
            clip_effects(&document)
                .iter()
                .map(|effect| effect.name.as_str())
                .collect::<Vec<_>>(),
            stack
                .iter()
                .map(|effect| effect.name.as_str())
                .collect::<Vec<_>>()
        );

        let report = qa_document(&document);
        let legacy_issues = report
            .issues
            .iter()
            .filter(|issue| issue.code == "legacy_lut_stage")
            .collect::<Vec<_>>();
        assert_eq!(
            legacy_issues.len(),
            1,
            "{label}: legacy_lut_stage must be reported exactly once"
        );
        assert_eq!(legacy_issues[0].severity, QaSeverity::Warning);
        assert!(legacy_issues[0].message.contains("cube_lut"));
        // The managed look never reports a legacy stage.
        assert!(
            !legacy_issues[0].message.contains("creative_look"),
            "{label}: a managed node must never be reported as a legacy stage"
        );

        let monitor = gpu_monitor(&compositor, resolution, &frame, stack, Some(luts.library()));
        rendered.push(monitor.clone());
        recorded.push(json!({
            "order": label,
            "effects": stack.iter().map(|effect| effect.name.as_str()).collect::<Vec<_>>(),
            "legacy_lut_stage_issues": legacy_issues.len(),
            "output_hash_sha256": output_hash(&monitor),
        }));
    }

    assert_eq!(
        rendered[0], rendered[1],
        "the legacy branch runs after every managed node, so the two vector orders must be \
         byte-identical"
    );
    assert_ne!(
        rendered[0], managed_only,
        "the legacy stage must actually change the frame, or the ordering claim is vacuous"
    );
    let legacy_only = gpu_monitor(
        &compositor,
        resolution,
        &frame,
        std::slice::from_ref(&legacy),
        Some(luts.library()),
    );
    assert_ne!(
        rendered[0], legacy_only,
        "the managed look must also change the frame, or only the legacy stage is being measured"
    );
    assert_ne!(legacy_only, managed_only);

    // §4.1: the legacy lattice occupies the last atlas slot, after the four
    // managed ones. Four managed nodes plus the legacy stage is the worst case.
    let four = mixed_size_lattices();
    let wide = FixtureLuts::build(
        "cc4-legacy-slots",
        &four.iter().map(lattice_cube_text).collect::<Vec<_>>(),
    );
    let mut full = (0..4)
        .map(|index| creative_look(index as u64 + 1, index + 1, linear, 10_000))
        .collect::<Vec<_>>();
    full.push(legacy.clone());
    let bytes = grade_buffer_bytes_with_luts(&full, Some(wide.library()))
        .expect("four managed nodes plus a legacy stage serialize");
    assert_eq!(
        grade_header_word(&bytes, 0),
        4,
        "the legacy stage is not a managed node"
    );
    for node in 0..4 {
        assert_eq!(
            grade_value(&bytes, node, 0),
            node as f32,
            "managed slots stay 0..=3"
        );
    }
    compositor
        .lut_binding(&full, Some(wide.library()))
        .expect("the legacy stage fits the fifth atlas slot");

    let metrics = json!({
        "lane": gpu.lane.id(),
        "orders": recorded,
        "orders_are_byte_identical": true,
        "legacy_slot": COMPOSITOR_LEGACY_LUT_SLOT,
        "atlas_slots": COMPOSITOR_LUT_ATLAS_SLOTS,
        "managed_only_output_hash_sha256": output_hash(&managed_only),
        "legacy_only_output_hash_sha256": output_hash(&legacy_only),
    });
    emit_cc4_evidence(
        "cc4_legacy_coexistence",
        gpu.backend(),
        gpu.lane.id(),
        json!({"section": "10.3.9"}),
        resolution,
        json_hash(&metrics),
        metrics,
    );
}

// ---------------------------------------------------------------------------
// §10.3.10: built-in bake determinism.
// ---------------------------------------------------------------------------

/// Every field of one canonical `.cube` line, asserted to be `{:.6}` fixed
/// decimal with no locale-dependent spelling.
fn assert_six_decimal_triple(line: &str, label: &str) {
    let fields = line.split(' ').collect::<Vec<_>>();
    assert_eq!(
        fields.len(),
        3,
        "{label}: {line:?} is not a three-value line"
    );
    for field in fields {
        let digits = field.strip_prefix('-').unwrap_or(field);
        let (integer, fraction) = digits
            .split_once('.')
            .unwrap_or_else(|| panic!("{label}: {field:?} is not a fixed decimal"));
        assert!(
            !integer.is_empty() && integer.bytes().all(|byte| byte.is_ascii_digit()),
            "{label}: {field:?} has no integer part"
        );
        assert_eq!(
            fraction.len(),
            6,
            "{label}: {field:?} must carry exactly six fractional digits"
        );
        assert!(
            fraction.bytes().all(|byte| byte.is_ascii_digit()),
            "{label}: {field:?} has a non-digit fraction"
        );
    }
}

/// CC4 §10.3.10. The five built-in bakes hash to their pinned literals, bake
/// byte-identically twice, carry the pinned LF `{:.6}` structure, and
/// reproduce their closed forms to within `2e-6` in display code on the CPU
/// reference and on the GPU.
#[test]
fn cc4_builtin_bakes_are_deterministic_and_reproduce_their_formulas() {
    let gpu = fallback_gpu();
    let compositor = Compositor::new(gpu.context());
    let (width, height, frame) = cc3_raster_frame();
    let resolution = (width, height);
    let samples = frame.pixels.as_chunks::<4>().0;

    assert_eq!(BuiltinLook::ALL.len(), 5);
    assert_eq!(BUILTIN_LOOK_SHA256.len(), 5);

    let mut recorded = Vec::new();
    for (index, look) in BuiltinLook::ALL.into_iter().enumerate() {
        let name = look.name();
        assert_eq!(BUILTIN_LOOK_SHA256[index].0, name, "catalogue order");
        assert_eq!(
            BuiltinLook::from_preset_token(index as i64),
            Some(look),
            "the legacy preset token mapping is normative"
        );

        // --- the pinned hash, re-derived from the canonical bytes -------
        let text = look.canonical_text();
        let derived = sha256_bytes(text.as_bytes());
        assert_eq!(
            derived,
            look.pinned_sha256(),
            "{name}: the bake no longer hashes to its pinned literal"
        );
        assert_eq!(derived, BUILTIN_LOOK_SHA256[index].1);
        assert_eq!(derived, output_hash(text.as_bytes()));
        assert_eq!(look.byte_len(), text.len() as u64);

        // --- two independent bakes are byte-identical -------------------
        let first = look.bake();
        let second = look.bake();
        assert_eq!(first, second, "{name}: two bakes must be byte-identical");
        assert_eq!(
            first.canonical_text(look.cube_title()),
            *text,
            "{name}: the cached bake and a fresh bake must serialize identically"
        );

        // --- the pinned serializer structure ---------------------------
        assert!(
            !text.contains('\r'),
            "{name}: the canonical text is LF only"
        );
        assert!(
            text.ends_with('\n'),
            "{name}: the canonical text ends with LF"
        );
        assert!(!text.ends_with("\n\n"), "{name}: no trailing blank line");
        let lines = text.lines().collect::<Vec<_>>();
        let size = look.size();
        let (minimum, maximum) = look.domain();
        assert_eq!(lines.len(), 4 + (size as usize).pow(3));
        assert_eq!(lines[0], format!("TITLE \"kinewright.look.{name}.v1\""));
        assert_eq!(lines[0], format!("TITLE \"{}\"", look.cube_title()));
        assert_eq!(lines[1], format!("LUT_3D_SIZE {size}"));
        assert_eq!(
            lines[2],
            format!("DOMAIN_MIN {m} {m} {m}", m = format_six(f64::from(minimum)))
        );
        assert_eq!(
            lines[3],
            format!("DOMAIN_MAX {m} {m} {m}", m = format_six(f64::from(maximum)))
        );
        for (offset, line) in lines[4..].iter().enumerate() {
            assert_six_decimal_triple(line, &format!("{name} sample line {}", offset + 5));
        }

        // The pinned text round-trips through the production parser in LF and
        // in CRLF form.
        let parsed = parse_cube_lut_typed(text).expect("the canonical text parses");
        assert_eq!(parsed.size, size);
        assert_eq!(parsed.title.as_deref(), Some(look.cube_title()));
        let crlf = parse_cube_lut_typed(&text.replace('\n', "\r\n")).expect("CRLF parses");
        assert_eq!(crlf, parsed);

        // --- the record ------------------------------------------------
        let asset = look.to_lut_asset(LutAssetId(index as u64 + 1));
        assert_eq!(asset.sha256, look.pinned_sha256());
        assert_eq!(asset.kind, LutAssetKind::Cube3d);
        assert_eq!(asset.size, size);
        assert_eq!(asset.byte_len, text.len() as u64);
        assert_eq!(
            asset.source,
            LutAssetSource::Builtin {
                name: name.to_owned()
            }
        );
        let expected_domain = if look == BuiltinLook::Identity {
            ([0_i64; 3], [1_000_000_i64; 3])
        } else {
            ([-1_000_000_i64; 3], [2_000_000_i64; 3])
        };
        assert_eq!(
            (asset.domain_min_millionths, asset.domain_max_millionths),
            expected_domain,
            "{name}: CC4 §2.6 bakes the four looks over [-1, 2] and identity over [0, 1]"
        );
        assert_eq!(validate_lut_asset(&asset), Ok(()));

        // --- the closed form on the §10.2 raster ------------------------
        // The fixture's own f64 transcription is checked against the
        // production formula first, so the comparison below is between two
        // implementations of §2.6 rather than one with itself.
        for probe in [[0.0, 0.0, 0.0], [0.18, 0.5, 0.9], [-0.7, 1.5, 2.0]] {
            let mine = spec_builtin_formula_f64(look, probe);
            let theirs = look.formula(probe);
            for channel in 0..3 {
                assert!(
                    (mine[channel] - theirs[channel]).abs() <= 1.0e-12,
                    "{name}: the fixture transcription of §2.6 disagrees with BuiltinLook::formula"
                );
            }
        }

        let (library, statuses) = LutLibrary::build(std::slice::from_ref(&asset), None);
        assert_eq!(statuses[0].1.kind, LutAvailabilityKind::Verified);
        let stack = [creative_look(
            1,
            index as i64 + 1,
            LutInputEncoding::Display709.token(),
            10_000,
        )];
        let nodes = cpu_nodes_with(&stack, &library);
        assert_eq!(nodes.len(), 1);

        // The closed-form expectation, per pixel, in display code and in
        // linear light.
        let mut expected_display = Vec::with_capacity(samples.len() * 3);
        let mut expected_linear_quantized = Vec::with_capacity(samples.len() * 4);
        for rgba in samples {
            let x = [
                f64::from(rgba[0].to_f32()),
                f64::from(rgba[1].to_f32()),
                f64::from(rgba[2].to_f32()),
            ];
            let e = x.map(spec_encode_bt709_signed_f64);
            let display = spec_builtin_formula_f64(look, e);
            expected_display.extend_from_slice(&display);
            for value in display {
                let linear = spec_decode_display709_f64(value) as f32;
                expected_linear_quantized.push(f16::from_f32(linear).to_f32());
            }
            expected_linear_quantized.push(f16::from_f32(rgba[3].to_f32()).to_f32());
        }

        // CPU: the unquantized reference against the closed form, in display
        // code, at the §10.3.10 gate.
        let mut cpu_display_error = 0.0_f64;
        for (pixel, rgba) in samples.iter().enumerate() {
            let out = apply_stack(
                &nodes,
                [rgba[0].to_f32(), rgba[1].to_f32(), rgba[2].to_f32()],
            );
            for channel in 0..3 {
                let actual = spec_encode_bt709_signed_f64(f64::from(out[channel]));
                let expected = expected_display[pixel * 3 + channel];
                cpu_display_error = cpu_display_error.max((actual - expected).abs());
            }
        }
        assert!(
            cpu_display_error <= BUILTIN_DISPLAY_CODE_TOLERANCE,
            "{name}: CPU display-code error {cpu_display_error} exceeds the §10.3.10 gate of \
             {BUILTIN_DISPLAY_CODE_TOLERANCE}; the affine reproduction claim is broken"
        );

        // GPU: the production `Rgba16Float` working surface quantizes its
        // output, and one f16 step at a display code of ~0.5 is 1e-4 — fifty
        // times the §10.3.10 gate — so the GPU is compared against the SAME
        // closed form carried through the SAME normative quantization, under
        // the CC1 §6.2 banded linear gate that exists for exactly this reason.
        // The display-code deviation is measured and recorded either way.
        let rendered = gpu_linear(&compositor, resolution, &frame, &stack, Some(&library));
        let parity = linear_parity_metrics(&rendered, &expected_linear_quantized);
        assert_linear_parity(&parity, &format!("builtin_{name}_gpu_vs_closed_form"));
        assert_eq!(parity.non_finite, 0);
        let mut gpu_display_error = 0.0_f64;
        for (pixel, rgb) in rendered.as_chunks::<4>().0.iter().enumerate() {
            for channel in 0..3 {
                let actual = spec_encode_bt709_signed_f64(f64::from(rgb[channel]));
                let expected = expected_display[pixel * 3 + channel];
                gpu_display_error = gpu_display_error.max((actual - expected).abs());
            }
        }

        // CPU against GPU under the ordinary §6.2 gates, so the two
        // implementations are compared directly as well.
        let cpu_quantized = cpu_reference_linear(&frame, &nodes);
        let cpu_gpu = linear_parity_metrics(&rendered, &cpu_quantized);
        assert_linear_parity(&cpu_gpu, &format!("builtin_{name}_cpu_vs_gpu"));

        recorded.push(json!({
            "name": name,
            "title": look.title(),
            "preset_token": index,
            "size": size,
            "domain": [minimum, maximum],
            "pinned_sha256": look.pinned_sha256(),
            "derived_sha256": derived,
            "byte_len": look.byte_len(),
            "canonical_lines": lines.len(),
            "two_bakes_identical": true,
            "cpu_max_display_code_error": cpu_display_error,
            "gpu_max_display_code_error": gpu_display_error,
            "gpu_vs_closed_form_linear": parity.as_json(),
            "cpu_vs_gpu_linear": cpu_gpu.as_json(),
        }));
    }

    let metrics = json!({
        "lane": gpu.lane.id(),
        "looks": recorded,
        "display_code_tolerance": BUILTIN_DISPLAY_CODE_TOLERANCE,
        "raster_rgb_samples": samples.len(),
        "note": "the GPU lane is gated against the same closed form carried through the \
                 normative Rgba16Float quantization, because one f16 step already exceeds the \
                 2e-6 display-code gate; the measured GPU display-code deviation is recorded",
    });
    emit_cc4_evidence(
        "cc4_builtin_bakes",
        gpu.backend(),
        gpu.lane.id(),
        json!({"section": "10.3.10", "looks": BuiltinLook::ALL.map(BuiltinLook::name)}),
        resolution,
        json_hash(&metrics),
        metrics,
    );
}

// ---------------------------------------------------------------------------
// §10.3.11 (media half) and §10.3.12: relocatability and recovery.
//
// The save/open half of §10.3.11 is owned by `crates/kinewright-app`, which is
// where `write_project` and the Save As store copy live. What is provable here
// — and what the app half depends on — is that the store root is derived from
// the project path at runtime, that a copied store reproduces the render
// bit-identically, and that a missing store blocks the render with the typed
// code instead of producing a look-free frame.
// ---------------------------------------------------------------------------

/// Copy a whole store directory tree.
fn copy_tree(source: &Path, destination: &Path) {
    fs::create_dir_all(destination).expect("the destination store root is created");
    for entry in fs::read_dir(source).expect("the source store root is readable") {
        let entry = entry.expect("a store entry is readable");
        let target = destination.join(entry.file_name());
        if entry
            .file_type()
            .expect("a store entry has a type")
            .is_dir()
        {
            copy_tree(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), &target).expect("a store file is copied");
        }
    }
}

/// A one-clip title timeline, so the CC4 library plumbing is exercised through
/// the production `FrameRenderer` without a decoder in the path.
fn relocatable_document(assets: &[LutAsset], effects: Vec<Effect>) -> Document {
    Document {
        resolution: (160, 90),
        duration: TimeCode(4),
        lut_assets: assets.to_vec(),
        tracks: vec![kinewright_core::Track {
            id: kinewright_core::TrackId(1),
            kind: kinewright_core::TrackKind::Video,
            sync_lock: true,
            clips: vec![kinewright_core::Clip {
                id: ClipId(1),
                asset: kinewright_core::AssetId::default(),
                source_range: TimeCode(0)..TimeCode(4),
                content: kinewright_core::ClipContent::Title(kinewright_core::Title {
                    text: "CC4".to_owned(),
                    ..kinewright_core::Title::default()
                }),
                timeline_start: TimeCode::ZERO,
                effects,
                transition_in: None,
                link: None,
                audio_gain_tenth_db: 0,
                audio_fade_in_frames: TimeCode::ZERO,
                audio_fade_out_frames: TimeCode::ZERO,
                speed_percent: 100,
            }],
        }],
        ..Document::default()
    }
}

/// Render one document through the production renderer with the supplied
/// library published, and hash the monitor raster.
fn published_render_hash(
    gpu: &crate::compositor::GpuContext,
    document: &Document,
    library: LutLibrary,
) -> Result<String, kinewright_core::MediaError> {
    let mut renderer = crate::render::FrameRenderer::new(gpu.clone());
    renderer.set_lut_library(Arc::new(library));
    let frame = renderer.render(
        document,
        TimeCode::ZERO,
        document.resolution,
        crate::render::RenderScale::FullResolution,
        crate::render::DecodeStrategy::Seek,
    )?;
    Ok(output_hash(frame.rgba.as_ref()))
}

/// CC4 §10.3.11, media half. The store root is derived from the project path,
/// a relocated store reproduces the render bit-identically, a missing store
/// blocks the render with `missing_lut_asset`, and `restore` / `copy_to`
/// return it to the same hash.
#[test]
fn cc4_relocating_the_store_reproduces_the_render_bit_identically() {
    let gpu = fallback_gpu();
    let context = gpu.context();

    // --- the original project ------------------------------------------
    let original_dir = TempDirectory::new("cc4-relocate-origin");
    let project = original_dir.path("edit.kinewright");
    let store = LutStore::for_project(&project).expect("a saved project derives a store root");
    assert_eq!(
        store.root(),
        original_dir.path("edit.kinewright-assets"),
        "the store root is <dir>/<stem>.kinewright-assets, derived and never stored"
    );
    // The stem rule is independent of the project extension.
    assert_eq!(
        LutStore::for_project(&original_dir.path("edit.json"))
            .expect("a .json project derives the same root")
            .root(),
        store.root()
    );

    let source_cube = original_dir.path("filmic.cube");
    fs::write(&source_cube, lattice_cube_text(&non_dyadic_look_lattice()))
        .expect("the source LUT is written");
    let import = store
        .import_lut_asset(&source_cube)
        .expect("the source LUT imports");
    assert_eq!(import.size, NON_DYADIC_LOOK_SIZE);
    let asset = import.into_lut_asset(LutAssetId(1));
    let expected_store_file = store
        .path_for(&asset.sha256)
        .expect("the store path is derived from the validated hash");
    assert_eq!(
        expected_store_file,
        store.luts_dir().join(format!("{}.cube", asset.sha256)),
        "the store file name is the content hash, so no user text reaches a path component"
    );
    assert!(expected_store_file.is_file());

    let assets = vec![asset.clone()];
    let document = relocatable_document(
        &assets,
        vec![creative_look(
            1,
            1,
            LutInputEncoding::Display709.token(),
            7_500,
        )],
    );

    let build_library = |store: Option<&LutStore>| {
        let (library, statuses) = LutLibrary::build(&assets, store);
        (library, statuses[0].1.clone())
    };

    let (library, status) = build_library(Some(&store));
    assert_eq!(status.kind, LutAvailabilityKind::Verified);
    assert_eq!(
        status.observed_sha256.as_deref(),
        Some(asset.sha256.as_str())
    );
    let original_hash = published_render_hash(&context, &document, library)
        .expect("the published library renders the look");

    // The same document without a look must hash differently, or the render
    // hash is not evidence of anything.
    let look_free = relocatable_document(&assets, Vec::new());
    let look_free_hash =
        published_render_hash(&context, &look_free, LutLibrary::default()).expect("look-free");
    assert_ne!(
        original_hash, look_free_hash,
        "the creative look must actually change the rendered frame"
    );

    // --- relocated: a different parent AND a different project stem ----
    let relocated_dir = TempDirectory::new("cc4-relocate-copy");
    let relocated_project = relocated_dir.path("renamed.kinewright");
    copy_tree(
        store.root(),
        &relocated_dir.path("renamed.kinewright-assets"),
    );
    let relocated_store =
        LutStore::for_project(&relocated_project).expect("the relocated project derives a root");
    assert_ne!(relocated_store.root(), store.root());
    let (relocated_library, relocated_status) = {
        let (library, statuses) = LutLibrary::build(&assets, Some(&relocated_store));
        (library, statuses[0].1.clone())
    };
    assert_eq!(relocated_status.kind, LutAvailabilityKind::Verified);
    let relocated_hash = published_render_hash(&context, &document, relocated_library)
        .expect("the relocated store renders");
    assert_eq!(
        relocated_hash, original_hash,
        "copying the project file and its <stem>.kinewright-assets directory must reproduce the \
         look bit-identically"
    );

    // --- without the store ---------------------------------------------
    let bare_dir = TempDirectory::new("cc4-relocate-bare");
    let bare_project = bare_dir.path("edit.kinewright");
    let bare_store = LutStore::for_project(&bare_project).expect("a bare project derives a root");
    let missing = bare_store.availability(&asset);
    assert_eq!(missing.kind, LutAvailabilityKind::Missing);
    assert_eq!(
        missing.path.as_deref(),
        Some(
            bare_store
                .luts_dir()
                .join(format!("{}.cube", asset.sha256))
                .as_path()
        ),
        "a missing asset must name the store path the operator has to restore into"
    );
    assert!(
        missing
            .reason
            .as_deref()
            .is_some_and(|reason| reason.contains("missing_lut_asset")),
        "the reason must carry the stable code: {:?}",
        missing.reason
    );
    let (bare_library, bare_status) = build_library(Some(&bare_store));
    assert_eq!(bare_status.kind, LutAvailabilityKind::Missing);
    assert!(bare_library.get(LutAssetId(1)).is_none());
    let error = published_render_hash(&context, &document, bare_library)
        .expect_err("a missing asset must block the render, never drop the node");
    let kinewright_core::MediaError::Backend(message) = &error else {
        panic!("expected a typed backend failure, got {error:?}");
    };
    assert!(
        message.starts_with("missing_lut_asset:"),
        "the render must fail with the stable code: {message}"
    );
    assert!(message.contains("creative_look"));

    // The library built with NO store root at all is the same blocking shape.
    let (no_store_library, no_store_status) = build_library(None);
    assert_eq!(no_store_status.kind, LutAvailabilityKind::Missing);
    assert!(
        published_render_hash(&context, &document, no_store_library).is_err(),
        "no store root must block just as a missing file does"
    );

    // --- restore returns it to the same bytes ---------------------------
    let restored_path = bare_store
        .restore(&asset, &source_cube)
        .expect("the original file restores");
    assert_eq!(
        restored_path,
        bare_store.luts_dir().join(format!("{}.cube", asset.sha256))
    );
    let (restored_library, restored_status) = build_library(Some(&bare_store));
    assert_eq!(restored_status.kind, LutAvailabilityKind::Verified);
    let restored_hash = published_render_hash(&context, &document, restored_library)
        .expect("the restored store renders");
    assert_eq!(
        restored_hash, original_hash,
        "restoring the recorded bytes must return the render to the first hash bit-identically"
    );

    // --- Save As into a third store -------------------------------------
    let saved_as_dir = TempDirectory::new("cc4-relocate-saveas");
    let saved_as_store = LutStore::for_project(&saved_as_dir.path("copy.kinewright"))
        .expect("the Save As target derives a root");
    let results = store.copy_to(&saved_as_store, &assets);
    assert_eq!(results.len(), 1);
    assert!(
        results[0].1.is_ok(),
        "Save As must copy the store file: {results:?}"
    );
    let (copied_library, copied_status) = build_library(Some(&saved_as_store));
    assert_eq!(copied_status.kind, LutAvailabilityKind::Verified);
    assert_eq!(
        published_render_hash(&context, &document, copied_library).expect("the copy renders"),
        original_hash
    );

    let metrics = json!({
        "lane": gpu.lane.id(),
        "asset": {
            "sha256": asset.sha256,
            "size": asset.size,
            "byte_len": asset.byte_len,
            "title": asset.title,
        },
        "store_roots": {
            "original": store.root().display().to_string(),
            "relocated": relocated_store.root().display().to_string(),
            "bare": bare_store.root().display().to_string(),
            "save_as": saved_as_store.root().display().to_string(),
        },
        "render_hash_sha256": {
            "original": original_hash,
            "relocated": relocated_hash,
            "restored": restored_hash,
            "look_free": look_free_hash,
        },
        "missing_render_error": message,
        "owner_of_save_open_half": "kinewright-app",
    });
    emit_cc4_evidence(
        "cc4_relocatable_store",
        gpu.backend(),
        gpu.lane.id(),
        json!({"section": "10.3.11", "half": "media"}),
        document.resolution,
        json_hash(&metrics),
        metrics,
    );
}

/// CC4 §10.3.12. A wrong restore candidate is refused and leaves the store
/// untouched, a corrupted store file reports `changed` and blocks, and
/// `RemoveLutAsset` is refused for an active, a bypassed, and a `Hold`-keyframed
/// reference alike — then succeeds once the referencing nodes are gone.
#[test]
fn cc4_recovery_rejections_are_typed_and_leave_the_store_untouched() {
    let gpu = fallback_gpu();
    let context = gpu.context();
    let directory = TempDirectory::new("cc4-recovery");
    let store = LutStore::for_project(&directory.path("edit.kinewright"))
        .expect("a saved project derives a store root");

    let source = directory.path("filmic.cube");
    fs::write(&source, lattice_cube_text(&non_dyadic_look_lattice())).expect("source written");
    let asset = store
        .import_lut_asset(&source)
        .expect("the source imports")
        .into_lut_asset(LutAssetId(1));
    let store_file = store.path_for(&asset.sha256).expect("store path");
    let stored_bytes = fs::read(&store_file).expect("the store file is readable");

    let assets = vec![asset.clone()];
    let document = relocatable_document(
        &assets,
        vec![creative_look(
            1,
            1,
            LutInputEncoding::Display709.token(),
            10_000,
        )],
    );

    // --- a different file is refused -----------------------------------
    let other = directory.path("other.cube");
    fs::write(&other, lattice_cube_text(&technical_lattice())).expect("the other LUT is written");
    let other_hash = sha256_bytes(&fs::read(&other).expect("readable"));
    let error = store
        .restore(&asset, &other)
        .expect_err("a different file must be refused");
    let kinewright_core::MediaError::Backend(message) = &error else {
        panic!("expected a typed backend failure, got {error:?}");
    };
    assert!(
        message.starts_with("lut_relink_hash_mismatch:"),
        "{message}"
    );
    assert!(
        message.contains(&other_hash),
        "the rejection must report the OBSERVED hash: {message}"
    );
    assert!(
        message.contains(&asset.sha256),
        "the rejection must report the EXPECTED hash: {message}"
    );
    assert_eq!(
        fs::read(&store_file).expect("the store file survives"),
        stored_bytes,
        "a refused restore must leave the store untouched"
    );
    assert_eq!(
        store.availability(&asset).kind,
        LutAvailabilityKind::Verified
    );

    // --- one corrupted byte ---------------------------------------------
    let mut corrupted = stored_bytes.clone();
    let flipped = corrupted
        .iter()
        .position(|byte| *byte == b'0')
        .expect("the canonical text has a zero digit");
    corrupted[flipped] = b'9';
    fs::write(&store_file, &corrupted).expect("the store file is corrupted");
    let observed = sha256_bytes(&corrupted);
    let changed = store.availability(&asset);
    assert_eq!(changed.kind, LutAvailabilityKind::Changed);
    assert_eq!(changed.observed_sha256.as_deref(), Some(observed.as_str()));
    assert!(
        changed
            .reason
            .as_deref()
            .is_some_and(|reason| reason.contains("changed_lut_asset")),
        "{:?}",
        changed.reason
    );
    let (changed_library, statuses) = LutLibrary::build(&assets, Some(&store));
    assert_eq!(statuses[0].1.kind, LutAvailabilityKind::Changed);
    assert!(
        changed_library.get(LutAssetId(1)).is_none(),
        "a changed asset must not be admitted to the library"
    );
    let blocked = published_render_hash(&context, &document, changed_library)
        .expect_err("a changed asset must block the render");
    let kinewright_core::MediaError::Backend(blocked_message) = &blocked else {
        panic!("expected a typed backend failure, got {blocked:?}");
    };
    assert!(
        blocked_message.starts_with("missing_lut_asset:"),
        "{blocked_message}"
    );

    // Restoring the recorded bytes repairs it.
    store
        .restore(&asset, &source)
        .expect("the original file restores");
    assert_eq!(fs::read(&store_file).expect("readable"), stored_bytes);
    assert_eq!(
        store.availability(&asset).kind,
        LutAvailabilityKind::Verified
    );

    // --- RemoveLutAsset is blocked by every kind of reference -----------
    let hold = AutomationCurve {
        keyframes: vec![
            Keyframe {
                at: TimeCode(0),
                value: 1,
                interpolation: KeyframeInterpolation::Hold,
            },
            Keyframe {
                at: TimeCode(2),
                value: 1,
                interpolation: KeyframeInterpolation::Hold,
            },
        ],
    };
    let mut hold_node = creative_look(3, 1, LutInputEncoding::Display709.token(), 10_000);
    hold_node
        .keyframes
        .insert("lut_asset_id".to_owned(), hold.clone());

    let references: [(&str, Effect); 3] = [
        (
            "active",
            creative_look(1, 1, LutInputEncoding::Display709.token(), 10_000),
        ),
        (
            "bypassed",
            with_parameter(
                &creative_look(2, 1, LutInputEncoding::Display709.token(), 10_000),
                "bypass",
                1,
            ),
        ),
        ("hold_keyframe", hold_node),
    ];

    let mut rejections = Vec::new();
    for (label, effect) in &references {
        let mut base = cc4_document();
        base.lut_assets = assets.clone();
        Operation::AddEffect {
            clip: ClipId(1),
            effect: effect.clone(),
        }
        .apply(&mut base)
        .unwrap_or_else(|error| panic!("{label} reference must be storable: {error}"));

        let mut candidate = base.clone();
        let error = Operation::RemoveLutAsset {
            lut_asset: LutAssetId(1),
        }
        .apply(&mut candidate)
        .expect_err("a referenced asset must not be removable");
        assert_eq!(
            error,
            OpError::LutAssetInUse {
                lut_asset: LutAssetId(1),
                clip: ClipId(1),
                effect: effect.id,
            },
            "{label}"
        );
        assert_eq!(candidate, base, "a rejection leaves the document untouched");
        assert_eq!(
            base.lut_asset_references(LutAssetId(1))
                .into_iter()
                .map(|(clip, effect)| (clip.0, effect.0))
                .collect::<Vec<_>>(),
            vec![(1, effect.id.0)],
            "{label}: the reference must be discoverable"
        );

        // Removing the node first lets the asset go.
        let mut freed = base.clone();
        Operation::RemoveEffect {
            clip: ClipId(1),
            effect: effect.id,
        }
        .apply(&mut freed)
        .expect("the referencing node is removable");
        Operation::RemoveLutAsset {
            lut_asset: LutAssetId(1),
        }
        .apply(&mut freed)
        .expect("an unreferenced asset is removable");
        assert!(freed.lut_assets.is_empty());
        // The store file is never deleted by a document edit (CC4 §2.4).
        assert!(
            store_file.is_file(),
            "{label}: removal must not touch the store"
        );
        rejections.push(json!({"reference": label, "error": error.to_string()}));
    }

    let metrics = json!({
        "lane": gpu.lane.id(),
        "expected_sha256": asset.sha256,
        "restore_mismatch_observed_sha256": other_hash,
        "corrupted_observed_sha256": observed,
        "changed_blocks_render": true,
        "remove_lut_asset_rejections": rejections,
        "store_file": store_file.display().to_string(),
    });
    emit_cc4_evidence(
        "cc4_recovery_rejections",
        gpu.backend(),
        gpu.lane.id(),
        json!({"section": "10.3.12"}),
        (0, 0),
        json_hash(&metrics),
        metrics,
    );
}

// ---------------------------------------------------------------------------
// §10.3.13: serialization, history, and typed rejections.
// ---------------------------------------------------------------------------

/// The actor's current document, read through the public query boundary.
fn query_document(core: &Core) -> Arc<Document> {
    match core
        .request(Command::Query(kinewright_core::Query::Document))
        .expect("the actor answers a document query")
    {
        Event::QueryResult(kinewright_core::QueryResult::Document(document)) => document,
        other => panic!("expected a document query result, got {other:?}"),
    }
}

/// The first CC4 §10.3.13 batch: register an asset and place a technical LUT
/// at an exact index. The legacy conversion and the automation follow as their
/// own batches in the test body, so undo and redo have several entries to walk.
fn history_operations(asset: &LutAsset) -> Vec<Operation> {
    vec![
        Operation::AddLutAsset {
            asset: asset.clone(),
        },
        Operation::InsertEffect {
            clip: ClipId(1),
            index: 0,
            effect: technical_lut(1, 1, LutInputEncoding::Grade709.token()),
        },
    ]
}

/// CC4 §10.3.13. Every new operation survives save/reopen, journal replay,
/// undo, and redo byte-for-byte, and a pre-CC4 project round-trips without a
/// `lut_assets` key.
#[test]
fn cc4_serialization_and_history_preserve_assets_and_nodes() {
    let directory = TempDirectory::new("cc4-history");
    let store =
        LutStore::for_project(&directory.path("edit.kinewright")).expect("store root derived");
    let source = directory.path("filmic.cube");
    fs::write(&source, lattice_cube_text(&technical_lattice())).expect("source written");
    let asset = store
        .import_lut_asset(&source)
        .expect("the source imports")
        .into_lut_asset(LutAssetId(1));

    // A pre-CC4 project has no `lut_assets` key at all and must re-serialize
    // without one.
    let pre_cc4 = cc4_document();
    let serialized = serde_json::to_value(&pre_cc4).expect("a pre-CC4 project serializes");
    assert!(
        serialized.get("lut_assets").is_none(),
        "an empty lut_assets must be skipped so pre-CC4 projects round-trip byte-unchanged"
    );
    let reopened: Document = serde_json::from_value(serialized).expect("it reopens");
    assert_eq!(reopened, pre_cc4);

    // --- the batch through the actor ------------------------------------
    let core = Core::spawn(cc4_document()).expect("cc4 history core");
    let base = cc4_document();
    let added = document_from(
        core.request(Command::DoBatch(history_operations(&asset)))
            .expect("the CC4 batch must be accepted"),
        "AddLutAsset + InsertEffect",
    );
    assert_eq!(added.lut_assets, vec![asset.clone()]);
    assert_eq!(added.next_lut_asset_id(), Ok(LutAssetId(2)));
    assert_eq!(added.lut_asset(LutAssetId(1)), Some(&asset));
    assert_eq!(
        clip_effects(&added)
            .iter()
            .map(|effect| effect.name.as_str())
            .collect::<Vec<_>>(),
        vec!["technical_lut"]
    );

    // A legacy look becomes a managed one in place, through the explicit
    // two-operation batch the contract names.
    let builtin = BuiltinLook::Warm.to_lut_asset(LutAssetId(2));
    let legacy = effect_with(
        2,
        "look_lut",
        &[("preset_token", 1), ("intensity_percent", 65)],
    );
    let with_legacy = document_from(
        core.request(Command::Do(Operation::AddEffect {
            clip: ClipId(1),
            effect: legacy.clone(),
        }))
        .expect("a legacy look is still storable"),
        "AddEffect look_lut",
    );
    assert_eq!(clip_effects(&with_legacy)[1].name, "look_lut");
    let converted = document_from(
        core.request(Command::DoBatch(vec![
            Operation::AddLutAsset {
                asset: builtin.clone(),
            },
            Operation::ConvertLegacyLook {
                clip: ClipId(1),
                effect: EffectId(2),
                lut_asset: LutAssetId(2),
                mix_basis_points: 6_500,
            },
        ]))
        .expect("[AddLutAsset, ConvertLegacyLook] must be accepted"),
        "ConvertLegacyLook",
    );
    assert_eq!(
        clip_effects(&converted)
            .iter()
            .map(|effect| (effect.id.0, effect.name.as_str()))
            .collect::<Vec<_>>(),
        vec![(1, "technical_lut"), (2, "creative_look")],
        "the converted node keeps its id and its exact vector position"
    );
    assert_eq!(
        LutNodeParams::from_effect(&clip_effects(&converted)[1]).mix_basis_points,
        6_500,
        "intensity_percent 65 converts to mix_basis_points 6500"
    );
    assert_eq!(converted.lut_assets[1], builtin);

    // Parameter edits and automation.
    let automated = document_from(
        core.request(Command::DoBatch(vec![
            Operation::SetEffectParam {
                clip: ClipId(1),
                effect: EffectId(2),
                name: "input_encoding_token".to_owned(),
                value: ParamValue::Integer(1),
            },
            Operation::SetEffectKeyframes {
                clip: ClipId(1),
                effect: EffectId(2),
                name: "mix_basis_points".to_owned(),
                curve: AutomationCurve {
                    keyframes: vec![
                        Keyframe {
                            at: TimeCode(0),
                            value: 0,
                            interpolation: KeyframeInterpolation::Linear,
                        },
                        Keyframe {
                            at: TimeCode(20),
                            value: 10_000,
                            interpolation: KeyframeInterpolation::EaseIn,
                        },
                    ],
                },
            },
            Operation::SetEffectKeyframes {
                clip: ClipId(1),
                effect: EffectId(1),
                name: "lut_asset_id".to_owned(),
                curve: AutomationCurve {
                    keyframes: vec![Keyframe {
                        at: TimeCode(0),
                        value: 1,
                        interpolation: KeyframeInterpolation::Hold,
                    }],
                },
            },
        ]))
        .expect("parameter and keyframe edits must be accepted"),
        "SetEffectParam + SetEffectKeyframes",
    );
    assert_eq!(
        clip_effects(&automated)[1].parameters["input_encoding_token"],
        ParamValue::Integer(1)
    );
    assert_eq!(
        clip_effects(&automated)[1].keyframes["mix_basis_points"]
            .keyframes
            .len(),
        2
    );

    // --- save and reopen -------------------------------------------------
    let saved = serde_json::to_vec(automated.as_ref()).expect("the CC4 document serializes");
    let reopened: Document = serde_json::from_slice(&saved).expect("the CC4 document reopens");
    assert_eq!(&reopened, automated.as_ref());
    let resaved = serde_json::to_vec(&reopened).expect("it re-serializes");
    assert_eq!(saved, resaved, "save/reopen must be byte-for-byte");
    let json: Value = serde_json::from_slice(&saved).expect("the JSON shape");
    let record = &json["lut_assets"][0];
    assert_eq!(record["id"], 1);
    assert_eq!(record["sha256"], asset.sha256);
    assert_eq!(record["kind"], "cube_3d");
    assert_eq!(record["size"], asset.size);
    assert_eq!(record["byte_len"], asset.byte_len);
    assert_eq!(
        json["lut_assets"][1]["source"],
        json!({"builtin": {"name": "warm"}})
    );
    assert!(
        record["source"]["imported"]["source_path"].is_string(),
        "imported provenance carries an informational source path"
    );

    // --- journal replay ---------------------------------------------------
    let replay_core = Core::spawn(cc4_document()).expect("cc4 replay core");
    let mut replayed = Arc::new(base.clone());
    for (label, journal) in [
        ("batch", JournalCommand::DoBatch(history_operations(&asset))),
        (
            "legacy",
            JournalCommand::Do(Operation::AddEffect {
                clip: ClipId(1),
                effect: legacy.clone(),
            }),
        ),
        (
            "convert",
            JournalCommand::DoBatch(vec![
                Operation::AddLutAsset {
                    asset: builtin.clone(),
                },
                Operation::ConvertLegacyLook {
                    clip: ClipId(1),
                    effect: EffectId(2),
                    lut_asset: LutAssetId(2),
                    mix_basis_points: 6_500,
                },
            ]),
        ),
    ] {
        let wire = serde_json::to_value(&journal).expect("a journal command serializes");
        let decoded: JournalCommand =
            serde_json::from_value(wire).expect("a journal command deserializes");
        replayed = document_from(
            replay_core
                .request(decoded.into())
                .unwrap_or_else(|error| panic!("replay of {label} failed: {error:?}")),
            label,
        );
    }
    assert_eq!(replayed.lut_assets, converted.lut_assets);
    assert_eq!(clip_effects(&replayed), clip_effects(&converted));

    // --- undo and redo -----------------------------------------------------
    let before_undo = serde_json::to_vec(automated.as_ref()).expect("serializes");
    let undone = document_from(core.request(Command::Undo).expect("undo"), "Undo");
    assert_eq!(
        clip_effects(&undone)[1].keyframes.get("mix_basis_points"),
        None,
        "undo must remove the automation the last batch added"
    );
    let redone = document_from(core.request(Command::Redo).expect("redo"), "Redo");
    assert_eq!(
        serde_json::to_vec(redone.as_ref()).expect("serializes"),
        before_undo,
        "redo must restore the document byte-for-byte"
    );

    // Undo all the way back and confirm the asset record leaves with it.
    // Four batches were accepted, and the undo/redo pair above left the
    // history at the newest entry.
    for _ in 0..4 {
        let _ = core.request(Command::Undo).expect("undo");
    }
    let empty = query_document(&core);
    assert!(empty.lut_assets.is_empty(), "undo removes the asset record");
    assert!(clip_effects(&empty).is_empty());
    assert_eq!(empty.as_ref(), &base);

    // Redo re-registers the same hash without touching the filesystem.
    for _ in 0..4 {
        let _ = core.request(Command::Redo).expect("redo");
    }
    let restored = query_document(&core);
    assert_eq!(
        serde_json::to_vec(restored.as_ref()).expect("serializes"),
        before_undo
    );

    // --- ClearEffectKeyframes and RemoveLutAsset ---------------------------
    let cleared = document_from(
        core.request(Command::Do(Operation::ClearEffectKeyframes {
            clip: ClipId(1),
            effect: EffectId(2),
            name: "mix_basis_points".to_owned(),
        }))
        .expect("clearing the mix automation is legal"),
        "ClearEffectKeyframes",
    );
    assert_eq!(
        clip_effects(&cleared)[1].keyframes.get("mix_basis_points"),
        None
    );
    let freed = document_from(
        core.request(Command::DoBatch(vec![
            Operation::RemoveEffect {
                clip: ClipId(1),
                effect: EffectId(2),
            },
            Operation::RemoveLutAsset {
                lut_asset: LutAssetId(2),
            },
        ]))
        .expect("removing the node then the asset is legal"),
        "RemoveLutAsset",
    );
    assert_eq!(freed.lut_assets, vec![asset.clone()]);
    assert!(store.path_for(&asset.sha256).expect("store path").is_file());

    let metrics = json!({
        "operations": [
            "AddLutAsset", "RemoveLutAsset", "InsertEffect", "ConvertLegacyLook",
            "SetEffectParam", "SetEffectKeyframes", "ClearEffectKeyframes",
        ],
        "save_reopen_byte_identical": true,
        "journal_replay_matches": true,
        "undo_redo_byte_identical": true,
        "pre_cc4_project_has_no_lut_assets_key": true,
        "document_sha256": output_hash(&saved),
    });
    emit_cc4_evidence(
        "cc4_serialization_history",
        CPU_REFERENCE_BACKEND,
        CPU_REFERENCE_LANE,
        json!({"section": "10.3.13"}),
        (0, 0),
        json_hash(&metrics),
        metrics,
    );
}

/// CC4 §10.3.13. Every illegal LUT edit is rejected atomically with `field`,
/// `observed`, and `allowed`, including the two the media layer owns:
/// `kind: cube_1d` at the operation layer and the hand-edited `size` the
/// hash-verified bytes contradict.
#[test]
fn cc4_illegal_lut_edits_are_rejected_atomically_with_field_observed_and_allowed() {
    let directory = TempDirectory::new("cc4-rejections");
    let store =
        LutStore::for_project(&directory.path("edit.kinewright")).expect("store root derived");
    let source = directory.path("filmic.cube");
    let text = lattice_cube_text(&non_dyadic_look_lattice());
    fs::write(&source, &text).expect("source written");
    let import = store.import_lut_asset(&source).expect("the source imports");
    let asset = import.into_lut_asset(LutAssetId(1));

    let mut base = cc4_document();
    Operation::AddLutAsset {
        asset: asset.clone(),
    }
    .apply(&mut base)
    .expect("the honest record is accepted");

    let mut recorded = Vec::new();
    let mut expect_metadata = |label: &str,
                               candidate: LutAsset,
                               field: &'static str,
                               observed: &str,
                               allowed: &'static str| {
        let mut document = base.clone();
        let error = Operation::AddLutAsset {
            asset: candidate.clone(),
        }
        .apply(&mut document)
        .expect_err("a malformed record must be rejected");
        assert_eq!(
            error,
            OpError::InvalidLutAssetMetadata {
                field,
                observed: observed.to_owned(),
                allowed,
            },
            "{label}"
        );
        assert_eq!(document, base, "{label}: a rejection is atomic");
        // The document-level invariant is the same rule as the operation.
        assert_eq!(validate_lut_asset(&candidate), Err(error.clone()));
        recorded.push(json!({
            "case": label,
            "field": field,
            "observed": observed,
            "allowed": allowed,
        }));
    };

    let with = |mutate: fn(&mut LutAsset)| {
        let mut candidate = LutAsset {
            id: LutAssetId(2),
            ..asset.clone()
        };
        candidate.sha256 = format!("{:0>64}", "b1");
        mutate(&mut candidate);
        candidate
    };
    expect_metadata(
        "kind_cube_1d",
        with(|candidate| candidate.kind = LutAssetKind::Cube1d),
        "kind",
        "cube_1d",
        "cube_3d",
    );
    expect_metadata(
        "byte_len_zero",
        with(|candidate| candidate.byte_len = 0),
        "byte_len",
        "0",
        "a positive byte length",
    );
    expect_metadata(
        "size_66",
        with(|candidate| candidate.size = 66),
        "size",
        "66",
        "2..=65",
    );
    expect_metadata(
        "size_1",
        with(|candidate| candidate.size = 1),
        "size",
        "1",
        "2..=65",
    );
    expect_metadata(
        "empty_title",
        with(|candidate| candidate.title = String::new()),
        "title",
        "",
        "a non-empty title",
    );

    // A malformed hash is its own typed variant, with the same shape.
    let mut document = base.clone();
    let malformed = LutAsset {
        id: LutAssetId(2),
        sha256: "NOT-A-DIGEST".to_owned(),
        ..asset.clone()
    };
    let error = Operation::AddLutAsset { asset: malformed }
        .apply(&mut document)
        .expect_err("a malformed hash must be rejected");
    assert_eq!(
        error,
        OpError::InvalidLutAssetHash {
            lut_asset: LutAssetId(2),
            observed: "NOT-A-DIGEST".to_owned(),
            allowed: "exactly 64 lowercase hexadecimal characters",
        }
    );
    assert_eq!(document, base);
    recorded.push(json!({
        "case": "malformed_hash",
        "field": "sha256",
        "observed": "NOT-A-DIGEST",
        "allowed": "exactly 64 lowercase hexadecimal characters",
    }));

    // A duplicate id.
    let mut document = base.clone();
    let error = Operation::AddLutAsset {
        asset: LutAsset {
            title: "A different look with the same id".to_owned(),
            ..asset.clone()
        },
    }
    .apply(&mut document)
    .expect_err("a duplicate id must be rejected");
    assert_eq!(error, OpError::DuplicateLutAsset(LutAssetId(1)));
    assert_eq!(document, base);

    // A dangling and an out-of-range asset reference.
    for (label, id, expected) in [
        (
            "dangling_lut_asset_id",
            99_i64,
            OpError::MissingLutAsset {
                clip: ClipId(1),
                effect: EffectId(7),
                lut_asset: LutAssetId(99),
            },
        ),
        (
            "lut_asset_id_above_the_json_safe_maximum",
            LUT_ASSET_ID_MAX as i64 + 1,
            OpError::EffectParamOutOfRange {
                effect: "creative_look".to_owned(),
                name: "lut_asset_id".to_owned(),
                min: 0,
                max: LUT_ASSET_ID_MAX as i64,
                actual: LUT_ASSET_ID_MAX as i64 + 1,
            },
        ),
    ] {
        let mut document = base.clone();
        let error = Operation::AddEffect {
            clip: ClipId(1),
            effect: creative_look(7, id, LutInputEncoding::Display709.token(), 10_000),
        }
        .apply(&mut document)
        .expect_err("an unusable asset reference must be rejected");
        assert_eq!(error, expected, "{label}");
        assert_eq!(document, base);
        recorded.push(json!({"case": label, "error": error.to_string()}));
    }

    // A `technical_lut` mix is pinned by its descriptor bounds.
    let mut document = base.clone();
    Operation::AddEffect {
        clip: ClipId(1),
        effect: technical_lut(1, 1, LutInputEncoding::Display709.token()),
    }
    .apply(&mut document)
    .expect("a bound technical LUT is legal");
    let pinned = document.clone();
    let error = Operation::SetEffectParam {
        clip: ClipId(1),
        effect: EffectId(1),
        name: "mix_basis_points".to_owned(),
        value: ParamValue::Integer(9_999),
    }
    .apply(&mut document)
    .expect_err("the technical mix is pinned at full strength");
    assert_eq!(
        error,
        OpError::EffectParamOutOfRange {
            effect: "technical_lut".to_owned(),
            name: "mix_basis_points".to_owned(),
            min: 10_000,
            max: 10_000,
            actual: 9_999,
        }
    );
    assert_eq!(document, pinned);
    let descriptor = effect_descriptor("technical_lut").expect("the descriptor exists");
    let mix = descriptor
        .parameters
        .iter()
        .find(|parameter| parameter.name == "mix_basis_points")
        .expect("the mix parameter exists");
    assert_eq!((mix.min, mix.max, mix.neutral), (10_000, 10_000, 10_000));
    recorded.push(json!({
        "case": "technical_lut_mix_9999",
        "field": "mix_basis_points",
        "observed": 9_999,
        "allowed": "10000..=10000",
    }));

    // `lut_asset_id` and `input_encoding_token` take Hold keyframes only.
    for name in ["lut_asset_id", "input_encoding_token"] {
        let mut candidate = pinned.clone();
        let error = Operation::SetEffectKeyframes {
            clip: ClipId(1),
            effect: EffectId(1),
            name: name.to_owned(),
            curve: AutomationCurve {
                keyframes: vec![
                    Keyframe {
                        at: TimeCode(0),
                        value: 1,
                        interpolation: KeyframeInterpolation::Hold,
                    },
                    Keyframe {
                        at: TimeCode(2),
                        value: 1,
                        interpolation: KeyframeInterpolation::Linear,
                    },
                ],
            },
        }
        .apply(&mut candidate)
        .expect_err("only hold keyframes are legal here");
        assert_eq!(
            error,
            OpError::NonHoldKeyframeParameter {
                effect: "technical_lut".to_owned(),
                name: name.to_owned(),
            }
        );
        assert_eq!(candidate, pinned);
        recorded.push(json!({
            "case": format!("non_hold_keyframe_{name}"),
            "field": name,
            "observed": "linear",
            "allowed": "hold",
        }));
    }

    // --- the media-owned mismatch: a hand-edited record ------------------
    // The bytes are hash-verified, so a disagreement can only mean the JSON
    // was edited by hand; it is a typed error, never a silent preference.
    let verified = parse_cube_lut_typed(&text).expect("the store bytes parse");
    assert_eq!(metadata_mismatch(&asset, &verified), None);
    let hand_edited = LutAsset {
        size: 17,
        ..asset.clone()
    };
    let (field, observed, allowed) =
        metadata_mismatch(&hand_edited, &verified).expect("a hand-edited size must be reported");
    assert_eq!(field, "size");
    assert_eq!(observed, "17");
    assert_eq!(allowed, NON_DYADIC_LOOK_SIZE.to_string());
    recorded.push(json!({
        "case": "lut_asset_metadata_mismatch_size",
        "field": field,
        "observed": observed,
        "allowed": allowed,
    }));

    // The library refuses the hand-edited record entirely rather than
    // rendering from a lossy mirror.
    let (library, statuses) = LutLibrary::build(std::slice::from_ref(&hand_edited), Some(&store));
    assert!(library.get(LutAssetId(1)).is_none());
    let reason = statuses[0]
        .1
        .reason
        .as_deref()
        .expect("a refused record reports why");
    assert!(
        reason.contains("lut_asset_metadata_mismatch"),
        "the library must report the stable code: {reason}"
    );

    // A hand-edited domain mirror is the same shape.
    let shifted = LutAsset {
        domain_max_millionths: [2_000_000; 3],
        ..asset.clone()
    };
    let (field, observed, allowed) =
        metadata_mismatch(&shifted, &verified).expect("a hand-edited domain must be reported");
    assert_eq!(field, "domain_max_millionths");
    assert_eq!(observed, "[2000000, 2000000, 2000000]");
    assert_eq!(allowed, "[1000000, 1000000, 1000000]");
    recorded.push(json!({
        "case": "lut_asset_metadata_mismatch_domain",
        "field": field,
        "observed": observed,
        "allowed": allowed,
    }));

    let metrics = json!({
        "rejections": recorded,
        "asset_sha256": asset.sha256,
        "lut_asset_id_max": LUT_ASSET_ID_MAX,
    });
    emit_cc4_evidence(
        "cc4_typed_rejections",
        CPU_REFERENCE_BACKEND,
        CPU_REFERENCE_LANE,
        json!({"section": "10.3.13"}),
        (0, 0),
        json_hash(&metrics),
        metrics,
    );
}

// ---------------------------------------------------------------------------
// §10.1.4 and §10.3: the manifest.
// ---------------------------------------------------------------------------

/// Every media-owned test this suite contains. The manifest may not name a
/// media test that is not in this list, so a renamed or deleted fixture is a
/// manifest failure rather than a silent gap.
const CC4_MEDIA_TESTS: [&str; 20] = [
    "cc4_cube_parsing_accepts_and_rejects_exactly_what_the_contract_lists",
    "cc4_input_encoding_tokens_are_hand_derived_and_dispatched",
    "cc4_identity_lattices_are_bit_exact_in_linear_on_cpu_and_gpu",
    "cc4_identity_round_trips_display709_and_grade709_within_the_linear_gate",
    "cc4_inactive_lut_nodes_are_bit_identical_to_the_stack_without_them",
    "cc4_interpolation_anchors_match_the_hand_derived_values",
    "cc4_out_of_domain_restores_the_excursion_and_stays_monotone",
    "cc4_mix_endpoints_and_midpoint_match_the_hand_derived_values",
    "cc4_stage_order_is_the_execution_order_and_a_violation_is_rejected",
    "cc4_parity_raster_exercises_the_out_of_domain_rule",
    "cc4_gpu_compositor_matches_the_cpu_reference_on_software_fallback",
    "cc4_gpu_compositor_matches_the_cpu_reference_on_hardware",
    "cc4_lut_slots_limits_and_abi_constants_hold",
    "cc4_legacy_cube_lut_runs_last_beside_a_managed_look",
    "cc4_builtin_bakes_are_deterministic_and_reproduce_their_formulas",
    "cc4_relocating_the_store_reproduces_the_render_bit_identically",
    "cc4_recovery_rejections_are_typed_and_leave_the_store_untouched",
    "cc4_serialization_and_history_preserve_assets_and_nodes",
    "cc4_illegal_lut_edits_are_rejected_atomically_with_field_observed_and_allowed",
    "cc4_manifest_declares_every_required_fixture_and_constant",
];

/// The two §10.3 items whose evidence lives outside this crate.
const CC4_EXTERNAL_OWNERS: [(u64, &str); 2] = [(11, "kinewright-app"), (14, "kinewright-agent")];

/// CC4 §10.1.4 and §10.3. Every required fixture is declared with its owner,
/// and every declared tolerance and constant is asserted equal to the code the
/// fixtures actually gate with.
#[test]
fn cc4_manifest_declares_every_required_fixture_and_constant() {
    let manifest: Value = serde_json::from_str(include_str!("../tests/fixtures/cc4_manifest.json"))
        .expect("CC4 fixture manifest must be valid JSON");
    assert_eq!(manifest["manifest_version"], 1);
    assert_eq!(manifest["contract"], "CC4 look management");
    assert_eq!(manifest["contract_token"], CC4_CONTRACT);
    assert_eq!(manifest["nodes"], json!(MANAGED_COLOR_NODE_NAMES));

    // --- §3.1 stage table ------------------------------------------------
    let stages = manifest["stages"]
        .as_array()
        .expect("the manifest must declare the stage table");
    assert_eq!(stages.len(), MANAGED_COLOR_NODE_NAMES.len());
    for (declared, name) in stages.iter().zip(MANAGED_COLOR_NODE_NAMES) {
        assert_eq!(declared["kind"], name);
        let kind = ColorNodeKind::from_effect_name(name)
            .unwrap_or_else(|| panic!("{name} must be a managed colour node"));
        assert_eq!(declared["rank"], u64::from(kind.stage().rank()));
        assert_eq!(declared["stage"], kind.stage().as_str());
        assert_eq!(
            declared["storage_buffer_tag"],
            u64::from(kind.storage_buffer_tag())
        );
    }

    // --- §5 control tables ------------------------------------------------
    for name in ["technical_lut", "creative_look"] {
        let descriptor = effect_descriptor(name).expect("the descriptor exists");
        let declared = manifest["lut_node_controls"][name]
            .as_array()
            .unwrap_or_else(|| panic!("the manifest must declare the {name} controls"));
        // CC5 §2.2 adds 47 `matte_*` parameters to `creative_look`'s
        // descriptor. They are CC5's table, declared by the CC5 manifest, so
        // this CC4 table counts the LUT controls only.
        let lut_controls = descriptor
            .parameters
            .iter()
            .filter(|parameter| !is_matte_parameter(parameter.name))
            .collect::<Vec<_>>();
        assert_eq!(
            lut_controls.len(),
            4,
            "{name} carries four CC4 LUT controls beside any CC5 matte parameters"
        );
        assert_eq!(declared.len(), lut_controls.len());
        for (row, parameter) in declared.iter().zip(lut_controls) {
            assert_eq!(row["name"], parameter.name, "{name}");
            assert_eq!(row["min"], parameter.min, "{name}.{}", parameter.name);
            assert_eq!(row["max"], parameter.max, "{name}.{}", parameter.name);
            assert_eq!(
                row["neutral"], parameter.neutral,
                "{name}.{}",
                parameter.name
            );
        }
    }

    // --- §3.4 encodings ---------------------------------------------------
    let encodings = manifest["input_encodings"]
        .as_array()
        .expect("the manifest must declare the encoding tokens");
    assert_eq!(encodings.len(), LutInputEncoding::ALL.len());
    for (declared, encoding) in encodings.iter().zip(LutInputEncoding::ALL) {
        assert_eq!(declared["token"], encoding.token());
        assert_eq!(declared["name"], encoding.as_str());
        assert_eq!(
            LutInputEncoding::from_token(encoding.token()),
            Some(encoding)
        );
    }

    // --- §2.1 asset model -------------------------------------------------
    let assets = &manifest["asset_model"];
    assert_eq!(assets["lut_size_min"], u64::from(MIN_CUBE_SIZE));
    assert_eq!(assets["lut_size_max"], u64::from(MAX_CUBE_SIZE));
    assert_eq!(
        assets["lut_size_min"],
        u64::from(kinewright_core::LUT_SIZE_MIN)
    );
    assert_eq!(
        assets["lut_size_max"],
        u64::from(kinewright_core::LUT_SIZE_MAX)
    );
    assert_eq!(assets["lut_asset_id_max"], LUT_ASSET_ID_MAX);
    assert_eq!(assets["lut_max_file_bytes"], LUT_MAX_FILE_BYTES);
    assert_eq!(
        assets["lut_node_limit_per_layer"],
        LUT_NODE_LIMIT_PER_LAYER as u64
    );
    assert_eq!(
        assets["color_node_limit_per_layer"],
        kinewright_core::COLOR_NODE_LIMIT_PER_LAYER as u64
    );
    assert_eq!(assets["store_suffix"], crate::lut_store::LUT_STORE_SUFFIX);
    assert_eq!(
        assets["store_luts_directory"],
        crate::lut_store::LUT_STORE_LUTS_DIRECTORY
    );

    // --- §2.6 pinned built-in hashes -------------------------------------
    let looks = manifest["builtin_looks"]
        .as_array()
        .expect("the manifest must declare the five built-in bakes");
    assert_eq!(looks.len(), BuiltinLook::ALL.len());
    for (index, (declared, look)) in looks.iter().zip(BuiltinLook::ALL).enumerate() {
        assert_eq!(declared["name"], look.name());
        assert_eq!(declared["title"], look.title());
        assert_eq!(declared["preset_token"], index as u64);
        assert_eq!(declared["size"], u64::from(look.size()));
        assert_manifest_f32(declared, "domain_min", look.domain().0);
        assert_manifest_f32(declared, "domain_max", look.domain().1);
        assert_eq!(
            declared["sha256"],
            look.pinned_sha256(),
            "the manifest must carry the pinned hash the code asserts"
        );
        assert_eq!(declared["sha256"], BUILTIN_LOOK_SHA256[index].1);
        // And the pin is the live bake, so a manifest hash can never outlive a
        // formula change.
        assert_eq!(
            declared["sha256"],
            sha256_bytes(look.canonical_text().as_bytes())
        );
    }

    // --- §4.1/§4.2 atlas and ABI constants --------------------------------
    let atlas = &manifest["atlas"];
    assert_eq!(
        atlas["compositor_lut_slots_per_layer"],
        COMPOSITOR_LUT_SLOTS_PER_LAYER as u64
    );
    assert_eq!(
        atlas["compositor_legacy_lut_slot"],
        COMPOSITOR_LEGACY_LUT_SLOT as u64
    );
    assert_eq!(
        atlas["compositor_lut_atlas_slots"],
        COMPOSITOR_LUT_ATLAS_SLOTS as u64
    );
    assert_eq!(
        atlas["compositor_required_texture_dimension_3d"],
        u64::from(COMPOSITOR_REQUIRED_TEXTURE_DIMENSION_3D)
    );
    assert_eq!(
        atlas["compositor_required_storage_buffer_binding_size"],
        COMPOSITOR_REQUIRED_STORAGE_BUFFER_BINDING_SIZE
    );
    assert_eq!(
        atlas["compositor_required_storage_buffers_per_shader_stage"],
        u64::from(COMPOSITOR_REQUIRED_STORAGE_BUFFERS_PER_SHADER_STAGE)
    );
    assert_eq!(
        atlas["worst_case_atlas_depth"],
        (COMPOSITOR_LUT_ATLAS_SLOTS * MAX_CUBE_SIZE as usize) as u64
    );
    // `GRADE_ABI_VERSION` is private to the compositor, so the manifest is
    // asserted against the version the production serializer actually writes
    // into `header.z` rather than against a restated literal.
    let empty = grade_buffer_bytes_with_luts(&[], None).expect("an empty stack serializes");
    assert_eq!(
        atlas["grade_abi_version"],
        u64::from(grade_header_word(&empty, 2))
    );
    assert_eq!(atlas["grade_abi_version"], 3);

    // --- §10.2 raster -----------------------------------------------------
    let raster = &manifest["raster"];
    assert_eq!(raster["rgb_samples"], 192);
    assert_eq!(
        raster["block_width_pixels"],
        u64::from(CC3_RASTER_BLOCK_WIDTH)
    );
    assert_eq!(raster["patterns"], json!(CC3_PATTERNS));
    let levels = raster["levels"]
        .as_array()
        .expect("the manifest must declare the raster levels");
    assert_eq!(levels.len(), CC3_RASTER_LEVELS.len());
    for (declared, expected) in levels.iter().zip(CC3_RASTER_LEVELS) {
        assert_eq!(
            declared.as_f64().expect("numeric raster level") as f32,
            expected,
            "manifest raster level does not match the code constant"
        );
    }
    assert_eq!(
        raster["minimum_samples_outside_unit_display"],
        MIN_OUT_OF_DOMAIN_RASTER_SAMPLES as u64
    );
    assert_eq!(
        raster["measured_samples_outside_unit_display"],
        raster_samples_outside_unit_display() as u64
    );

    // --- §10.1.4 tolerances are the CC1 §6.2 code constants ---------------
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
        "builtin_display_code_tolerance",
        BUILTIN_DISPLAY_CODE_TOLERANCE,
    );
    assert_manifest_f64(
        tolerances,
        "minimum_changed_linear_basis_points",
        MIN_CHANGED_LINEAR_BASIS_POINTS as f64,
    );

    // --- the fixture inventory --------------------------------------------
    assert_eq!(
        manifest["required_evidence"],
        json!(CC4_EVIDENCE_FIXTURES),
        "the manifest evidence list must match the emitted fixture names exactly"
    );
    let items = manifest["required_fixtures"]
        .as_array()
        .expect("the manifest must map the §10.3 items to owners and test names");
    assert_eq!(items.len(), 14, "§10.3 lists fourteen required fixtures");
    let mut declared_media_tests = Vec::new();
    for (index, item) in items.iter().enumerate() {
        let number = index as u64 + 1;
        assert_eq!(item["item"], number);
        assert!(
            item["name"].as_str().is_some_and(|name| !name.is_empty()),
            "§10.3 item {number} must be named"
        );
        let owners = item["owners"]
            .as_array()
            .unwrap_or_else(|| panic!("§10.3 item {number} must declare its owners"));
        assert!(!owners.is_empty(), "§10.3 item {number} must have an owner");
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
                "§10.3 item {number} names an unknown owner {crate_name}"
            );
            let tests = owner["tests"]
                .as_array()
                .unwrap_or_else(|| panic!("§10.3 item {number} must name its tests"));
            assert!(
                !tests.is_empty(),
                "§10.3 item {number} owner {crate_name} must name at least one test"
            );
            assert!(
                owner["scope"]
                    .as_str()
                    .is_some_and(|scope| !scope.is_empty()),
                "§10.3 item {number} owner {crate_name} must state what it covers"
            );
            if crate_name == "kinewright-media" {
                for test in tests {
                    let name = test.as_str().expect("a test name");
                    assert!(
                        CC4_MEDIA_TESTS.contains(&name),
                        "§10.3 item {number} names media test {name}, which this file does not \
                         contain"
                    );
                    declared_media_tests.push(name.to_owned());
                }
            }
        }
    }
    // The manifest must account for every media test, not merely a subset.
    // The inventory test itself is §10.1.4 rather than a §10.3 item, so it is
    // declared separately below.
    declared_media_tests
        .push("cc4_manifest_declares_every_required_fixture_and_constant".to_owned());
    for name in CC4_MEDIA_TESTS {
        assert!(
            declared_media_tests.iter().any(|declared| declared == name),
            "media test {name} is not claimed by any §10.3 item in the manifest"
        );
    }
    assert_eq!(
        manifest["manifest_self_test"]["test"],
        "cc4_manifest_declares_every_required_fixture_and_constant",
        "the manifest must name the test that asserts it against the code"
    );
    assert!(
        manifest["manifest_self_test"]["rule"]
            .as_str()
            .is_some_and(|rule| rule.contains("10.1.4")),
        "the manifest must cite the fixture-quality rule it satisfies"
    );

    // The two items this crate does not own must say so by name.
    for (number, owner) in CC4_EXTERNAL_OWNERS {
        let item = &items[number as usize - 1];
        assert!(
            item["owners"]
                .as_array()
                .expect("owners")
                .iter()
                .any(|entry| entry["owner"] == owner),
            "§10.3 item {number} must record {owner} as an owner"
        );
    }

    for lane in ["software", "software_unavailable_opt_in", "hardware"] {
        assert!(
            manifest["gpu_contexts"][lane].is_string(),
            "the manifest must describe the {lane} GPU lane"
        );
    }
}

// ---------------------------------------------------------------------------
// §5 / §3.4: the `input_encoding_token` control.
// ---------------------------------------------------------------------------

/// CC4 §10.1 rule 2 for `input_encoding_token`: every one of the three tokens
/// has a hand-derived numeric expected value, the token reaches the shader
/// record, and the tokens are proved to dispatch to different transfer pairs.
///
/// The separation between `display709` and `grade709` is measured and recorded
/// rather than asserted large: the two parameterizations are near-identical
/// analytic bijections, so on a smooth lattice they agree to `~1e-8`. The
/// discriminating case below therefore uses the non-dyadic 33³ look at the
/// raster's over-range extreme, where the measured separation is `2.3e-4` —
/// twenty times the CPU tolerance, and the only place in this suite where a
/// token-2-to-token-0 mis-dispatch would actually show.
#[test]
fn cc4_input_encoding_tokens_are_hand_derived_and_dispatched() {
    /// f32 round-off through the `2.2222` decode exponent is `~2e-7`
    /// relative; this is two orders tighter and still far above it.
    const CPU_TOLERANCE: f32 = 1.0e-6;
    /// The over-range discriminating case reaches `~3.09`, where the same
    /// relative round-off is `~4e-7`.
    const CPU_TOLERANCE_OVER_RANGE: f32 = 1.0e-5;

    let gpu = fallback_gpu();
    let compositor = Compositor::new(gpu.context());
    let b = lut_b_lattice();
    let luts = FixtureLuts::build(
        "cc4-encodings",
        &[
            lattice_cube_text(&b),
            lattice_cube_text(&non_dyadic_look_lattice()),
        ],
    );

    // `x` is exactly representable in f16, so the working frame hands the node
    // exactly these values.
    const INPUT: [f32; 3] = [0.5, 0.25, 0.125];
    // `e = ENC(x)`, `y = tetrahedral(LUT B, e)` (branch 1 in every case, since
    // `e_r > e_g > e_b` for all three encodings), `out = DEC(y)`:
    //
    //   linear      e = (0.500000, 0.250000, 0.125000)
    //               y = (0.312500, 0.187500, 0.125000)
    //   display709  e = (0.705515, 0.489940, 0.332129)
    //               y = (0.518822, 0.411034, 0.332129)
    //   grade709    e = (0.705436, 0.489802, 0.331949)
    //               y = (0.518692, 0.410875, 0.331949)
    const EXPECTED: [(&str, i64, [f32; 3]); 3] = [
        ("linear", 1, [0.312_500_00, 0.187_500_00, 0.125_000_00]),
        ("display709", 0, [0.278_064_78, 0.181_599_54, 0.125_000_00]),
        ("grade709", 2, [0.278_064_77, 0.181_599_53, 0.124_999_99]),
    ];

    let (width, height, frame) = anchor_frame(&[INPUT]);
    let resolution = (width, height);
    let mut outputs: Vec<(&str, [f32; 3])> = Vec::new();
    let mut recorded = Vec::new();
    for (name, token, expected) in EXPECTED {
        let encoding = LutInputEncoding::from_token(token)
            .unwrap_or_else(|| panic!("{token} is a documented encoding token"));
        assert_eq!(encoding.as_str(), name);

        // The fixture's own f64 transcription of §3.5 must agree with the
        // literal above before either is compared against production.
        let spec = b.apply(encoding, 1.0, INPUT.map(f64::from));
        for channel in 0..3 {
            assert!(
                (spec[channel] - f64::from(expected[channel])).abs() <= 1.0e-7,
                "{name}: the f64 transcription disagrees with the written anchor: {spec:?}"
            );
        }

        let stack = [creative_look(1, 1, token, 10_000)];
        let bytes = grade_buffer_bytes_with_luts(&stack, Some(luts.library()))
            .expect("the node serializes");
        assert_eq!(
            grade_value(&bytes, 0, 2).to_bits(),
            (token as f32).to_bits(),
            "{name}: the token must reach the shader record unchanged"
        );

        // The unquantized CPU reference: `INPUT` is exactly representable in
        // f16, so this is the very value the working frame hands the node, and
        // the comparison is not limited by the `Rgba16Float` storage step the
        // way a readback would be.
        let cpu = apply_stack(&cpu_nodes_with(&stack, luts.library()), INPUT);
        assert_rgb_within(cpu, expected, CPU_TOLERANCE, &format!("{name}: CPU"));
        let rendered = block_rgb(
            &gpu_linear(
                &compositor,
                resolution,
                &frame,
                &stack,
                Some(luts.library()),
            ),
            0,
        );
        assert_rgb_within(rendered, expected, ANCHOR_GATE, &format!("{name}: GPU"));

        outputs.push((name, cpu));
        recorded.push(json!({
            "encoding": name,
            "token": token,
            "input": INPUT,
            "expected": expected,
            "cpu": cpu,
            "gpu": rendered,
        }));
    }

    // `linear` must be visibly a different transfer from either 709 pair.
    for (name, output) in &outputs[1..] {
        let separation = (0..3)
            .map(|channel| (outputs[0].1[channel] - output[channel]).abs())
            .fold(0.0_f32, f32::max);
        assert!(
            separation > ANCHOR_GATE,
            "linear and {name} produced the same picture ({separation}); the encoding token is \
             not being dispatched"
        );
    }

    // --- display709 against grade709, where they actually separate --------
    // The §10.2 raster's over-range extreme through the non-dyadic 33³ look.
    const OVER_RANGE: [f32; 3] = [4.0, 0.0, 0.0];
    let look = non_dyadic_look_lattice().quantized_like_cube_text();
    let (or_width, or_height, or_frame) = anchor_frame(&[OVER_RANGE]);
    let mut over_range = Vec::new();
    for (name, token) in [("display709", 0_i64), ("grade709", 2)] {
        let encoding = LutInputEncoding::from_token(token).expect("a documented token");
        let spec = look.apply(encoding, 1.0, OVER_RANGE.map(f64::from));
        let stack = [creative_look(1, 2, token, 10_000)];
        let cpu = apply_stack(&cpu_nodes_with(&stack, luts.library()), OVER_RANGE);
        let expected = spec.map(|value| value as f32);
        assert_rgb_within(
            cpu,
            expected,
            CPU_TOLERANCE_OVER_RANGE,
            &format!("{name}: over-range CPU"),
        );
        let rendered = block_rgb(
            &gpu_linear(
                &compositor,
                (or_width, or_height),
                &or_frame,
                &stack,
                Some(luts.library()),
            ),
            0,
        );
        // The f16 working surface quantizes a value near 3.09 in steps of
        // 2^-9, so the GPU is held to the §6.2 band rather than to the CPU
        // tolerance.
        assert_rgb_within(
            rendered,
            expected,
            LINEAR_OVER_RANGE_P99 * 2.0,
            &format!("{name}: over-range GPU"),
        );
        over_range.push((name, cpu, expected));
    }
    let separation = (0..3)
        .map(|channel| (over_range[0].1[channel] - over_range[1].1[channel]).abs())
        .fold(0.0_f32, f32::max);
    assert!(
        separation > CPU_TOLERANCE_OVER_RANGE * 10.0,
        "display709 and grade709 produced indistinguishable output ({separation}); token 2 is \
         not dispatching to the grade709 transfer pair"
    );

    let metrics = json!({
        "lane": gpu.lane.id(),
        "anchors": recorded,
        "cpu_tolerance": CPU_TOLERANCE,
        "over_range": {
            "input": OVER_RANGE,
            "look_size": NON_DYADIC_LOOK_SIZE,
            "display709": over_range[0].1,
            "grade709": over_range[1].1,
            "measured_separation": separation,
            "cpu_tolerance": CPU_TOLERANCE_OVER_RANGE,
        },
    });
    emit_cc4_evidence(
        "cc4_input_encodings",
        gpu.backend(),
        gpu.lane.id(),
        json!({"section": "10.1.2", "tokens": [0, 1, 2]}),
        resolution,
        json_hash(&metrics),
        metrics,
    );
}
