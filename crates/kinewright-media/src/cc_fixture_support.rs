//! Helpers shared by the `ccN_fixtures` evidence modules.
//!
//! Each `ccN_fixtures.rs` used to carry its own copy of these. The copies had
//! drifted only in doc comments, constant names, and panic messages — never in
//! behaviour — and a copy that *can* drift silently is exactly what the
//! contracts' reuse rules (CC3 §10, CC4 §10, CC5 §9) exist to prevent. Every
//! item here is one of:
//!
//! * a **transcription** — the `SPEC_GRADE709_*` digits and the `spec_*_f64`
//!   functions are hand transcriptions of the written contract (CC3 §2.1's
//!   `grade709` pair, the BT.709 luma coefficients, `smoothstep`), permitted
//!   once by the fixture-quality rules' transcription clause (CC6 rule 11.0.1
//!   and its CC3 §10.1.1 / CC4 §10.1 / CC5 §9.0 counterparts). None of them
//!   calls `color_pipeline` or any other production module, so an expectation
//!   computed here can still detect a change to the implementation under test;
//! * the **CPU reference** the GPU parity gates compare against —
//!   [`cpu_reference_linear`] and [`cpu_reference_monitor`] evaluate the
//!   resolved node stack through `apply_color_nodes_at`, exactly as the owning
//!   fixtures already did, so the GPU is measured against the CPU evaluator
//!   rather than against itself;
//! * a thin wrapper over a **production render entry point** ([`gpu_linear`],
//!   [`gpu_monitor`]), or a test-harness convenience — document plumbing, the
//!   manifest integer check, and the declared-test inventory helpers.
//!
//! Nothing in this module is a `#[test]`; the fixtures that own the assertions
//! still live in the `ccN_fixtures` files, and a helper whose behaviour
//! genuinely differs between two contracts (CC1's `PrimaryCorrection`
//! references, CC3's LUT-less render wrappers) stays with its owner.

#![allow(clippy::cast_precision_loss)]

use std::{collections::BTreeMap, sync::Arc};

use half::f16;
use kinewright_core::{ColorContext, Document, Effect, EffectId, Event, ParamValue};
use serde_json::Value;

use crate::{
    Compositor, CompositorLayer,
    color_pipeline::{
        ColorNode, apply_color_nodes_at, encode_monitor_rgba8, resolve_color_nodes,
        resolve_color_nodes_with,
    },
    frame::WorkingFrame,
    lut_store::LutLibrary,
    timeline::TransitionRenderParams,
};

// ---------------------------------------------------------------------------
// Independent f64 transcriptions of the written contracts.
//
// Nothing below calls the production crate. The constants are the CC3 §2.1
// digits and the algorithms are the contract pseudocode, transcribed by hand,
// so a parity or boundary assertion compares two implementations of the
// written contract rather than one implementation with itself.
// ---------------------------------------------------------------------------

pub(crate) const SPEC_GRADE709_ALPHA: f64 = 1.099_296_8;
pub(crate) const SPEC_GRADE709_BETA: f64 = 0.018_053_969;
pub(crate) const SPEC_GRADE709_BETA_ENCODED: f64 = 0.081_242_86;
pub(crate) const SPEC_GRADE709_K: f64 = 0.099_296_8;
pub(crate) const SPEC_GRADE709_SLOPE: f64 = 4.5;
pub(crate) const SPEC_GRADE709_EXPONENT: f64 = 0.45;
pub(crate) const SPEC_GRADE709_INVERSE_EXPONENT: f64 = 2.222_222_3;

/// `sgn` with `sgn(0) = 0`, the CC3 definition WGSL's `sign` also matches;
/// `f64::signum` returns `±1` at zero and would break the bit-exact identity
/// CC4 §10.3.2 needs.
pub(crate) fn spec_sign_f64(value: f64) -> f64 {
    if value > 0.0 {
        1.0
    } else if value < 0.0 {
        -1.0
    } else {
        0.0
    }
}

/// CC3 §2.1's `grade709_encode`, in f64.
pub(crate) fn spec_grade709_encode_f64(x: f64) -> f64 {
    let sign = spec_sign_f64(x);
    let magnitude = x.abs();
    if magnitude < SPEC_GRADE709_BETA {
        sign * SPEC_GRADE709_SLOPE * magnitude
    } else {
        sign * (SPEC_GRADE709_ALPHA * magnitude.powf(SPEC_GRADE709_EXPONENT) - SPEC_GRADE709_K)
    }
}

/// CC3 §2.1's `grade709_decode`, the exact analytic inverse of
/// [`spec_grade709_encode_f64`], in f64.
pub(crate) fn spec_grade709_decode_f64(e: f64) -> f64 {
    let sign = spec_sign_f64(e);
    let magnitude = e.abs();
    if magnitude < SPEC_GRADE709_BETA_ENCODED {
        sign * magnitude / SPEC_GRADE709_SLOPE
    } else {
        sign * ((magnitude + SPEC_GRADE709_K) / SPEC_GRADE709_ALPHA)
            .powf(SPEC_GRADE709_INVERSE_EXPONENT)
    }
}

/// Rec.709 luma, `0.2126 R + 0.7152 G + 0.0722 B`, transcribed from the
/// contract (CC4 §2.6).
pub(crate) fn spec_luma_f64(rgb: [f64; 3]) -> f64 {
    0.2126 * rgb[0] + 0.7152 * rgb[1] + 0.0722 * rgb[2]
}

/// GLSL/WGSL `smoothstep`, transcribed in f64.
pub(crate) fn spec_smoothstep_f64(start: f64, end: f64, value: f64) -> f64 {
    let t = ((value - start) / (end - start)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

// ---------------------------------------------------------------------------
// CPU reference and GPU rendering.
// ---------------------------------------------------------------------------

/// The resolved node stack of `effects`, with no LUT library.
pub(crate) fn cpu_nodes(effects: &[Effect]) -> Vec<ColorNode> {
    resolve_color_nodes(effects).expect("fixture node stack must resolve")
}

/// The resolved node stack of `effects` against a verified LUT library.
pub(crate) fn cpu_nodes_with(effects: &[Effect], library: &LutLibrary) -> Vec<ColorNode> {
    resolve_color_nodes_with(effects, library).expect("fixture node stack must resolve")
}

/// The CC5 §3.4 pixel-centre uv of raster index `index`,
/// `((x + 0.5) / W, (y + 0.5) / H)`, matching the rasterizer's
/// `@builtin(position)` convention.
pub(crate) fn pixel_centre_uv(frame: &WorkingFrame, index: usize) -> [f32; 2] {
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
pub(crate) fn raster_aspect(frame: &WorkingFrame) -> f32 {
    frame.width.max(1) as f32 / frame.height.max(1) as f32
}

/// The independent CPU reference in the linear working domain, including the
/// normative `Rgba16Float` storage quantization.
pub(crate) fn cpu_reference_linear(frame: &WorkingFrame, nodes: &[ColorNode]) -> Vec<f32> {
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

/// The independent CPU reference on monitor codes: the linear reference,
/// quantized to `Rgba16Float`, then encoded with the monitoring transfer.
pub(crate) fn cpu_reference_monitor(frame: &WorkingFrame, nodes: &[ColorNode]) -> Vec<u8> {
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

/// The CC5 §3.4 reference at the centre of a square raster.
///
/// No CC3 or CC4 node stack carries a matte, so the position and the aspect
/// are immaterial and the result is bit-identical to the pre-CC5 positionless
/// reference — which is the point of CC5 §2.5's mandatory matte-free branch.
pub(crate) fn apply_stack(nodes: &[ColorNode], rgb: [f32; 3]) -> [f32; 3] {
    apply_color_nodes_at(nodes, rgb, [0.5, 0.5], 1.0)
}

/// The production working-surface render of one layer, through
/// `Compositor::render_working_with_luts`.
pub(crate) fn gpu_linear(
    compositor: &Compositor,
    resolution: (u32, u32),
    frame: &WorkingFrame,
    effects: &[Effect],
    library: Option<&LutLibrary>,
) -> Vec<f32> {
    compositor
        .render_working_with_luts(
            resolution,
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

/// The production monitor render of one layer, through
/// `Compositor::render_monitor_with_luts` with the SDR Rec.709 monitoring
/// description.
pub(crate) fn gpu_monitor(
    compositor: &Compositor,
    resolution: (u32, u32),
    frame: &WorkingFrame,
    effects: &[Effect],
    library: Option<&LutLibrary>,
) -> Vec<u8> {
    compositor
        .render_monitor_with_luts(
            resolution,
            &[CompositorLayer {
                frame,
                effects,
                transition: TransitionRenderParams::default(),
            }],
            &ColorContext::sdr_rec709().monitoring,
            library,
        )
        .expect("production GPU compositor should render the fixture")
        .rgba
        .as_ref()
        .clone()
}

/// Little-endian word `index` of a grade-buffer header.
pub(crate) fn grade_header_word(bytes: &[u8], index: usize) -> u32 {
    let offset = index * 4;
    u32::from_le_bytes(bytes[offset..offset + 4].try_into().expect("header word"))
}

// ---------------------------------------------------------------------------
// Document plumbing.
// ---------------------------------------------------------------------------

/// One colour node (`color_wheels`, `primary_correction`, …) with integer
/// parameters.
pub(crate) fn effect_with(id: u64, name: &str, parameters: &[(&str, i64)]) -> Effect {
    Effect {
        id: EffectId(id),
        name: name.to_owned(),
        parameters: parameters
            .iter()
            .map(|(name, value)| ((*name).to_owned(), ParamValue::Integer(*value)))
            .collect(),
        keyframes: BTreeMap::new(),
    }
}

/// `effect` with one integer parameter set.
pub(crate) fn with_parameter(effect: &Effect, name: &str, value: i64) -> Effect {
    let mut updated = effect.clone();
    updated
        .parameters
        .insert(name.to_owned(), ParamValue::Integer(value));
    updated
}

/// The effects of the single clip on the single track.
pub(crate) fn clip_effects(document: &Document) -> &[Effect] {
    &document.tracks[0].clips[0].effects
}

/// The document an accepted command produced, or a panic naming `label`.
pub(crate) fn document_from(event: Event, label: &str) -> Arc<Document> {
    match event {
        Event::DocumentChanged { doc, .. } => doc,
        other => panic!("{label} was not an accepted document state: {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// Manifest and declared-test inventory helpers.
// ---------------------------------------------------------------------------

/// Assert one declared manifest integer equals the code constant the fixtures
/// actually gate with.
///
/// The `i64` sibling of CC1's `assert_manifest_f64`: every threshold that is
/// an integer constant — millionths, hundredths, basis points, code bounds —
/// is asserted exactly, with no float round trip that could hide a one-unit
/// drift.
pub(crate) fn assert_manifest_i64(parent: &Value, key: &str, expected: i64) {
    let declared = parent
        .get(key)
        .and_then(Value::as_i64)
        .unwrap_or_else(|| panic!("manifest must declare an integer {key}"));
    assert_eq!(
        declared, expected,
        "manifest {key} does not match the code constant"
    );
}

pub(crate) fn sorted(names: impl IntoIterator<Item = String>) -> Vec<String> {
    let mut names = names.into_iter().collect::<Vec<_>>();
    names.sort_unstable();
    names
}

/// Whether `source` *uses* `needle` as code rather than merely naming it in a
/// comment or a message.
///
/// A call is the identifier followed by `(`, on a line that is not a comment,
/// with any trailing `//` comment stripped first. Exempting every line that
/// contains a string literal would let `fixture_gpu_or_skip("cc6-verify")` —
/// the natural spelling — evade the guard, so string literals are not exempt;
/// only the identifier-plus-paren shape counts, and prose mentions inside
/// quotes never carry the paren directly after the name. The quoted form is
/// the `std::env::var("NAME")` shape — the needle directly inside a call's
/// parentheses — so a fixture's own needle list, written as an array literal
/// (`["…", "…"]`), cannot match itself.
pub(crate) fn uses_outside_prose(source: &str, needle: &str) -> bool {
    let call = format!("{needle}(");
    let quoted = format!("(\"{needle}\")");
    source.lines().any(|line| {
        let code = line.split("//").next().unwrap_or_default();
        code.contains(&call) || code.contains(&quoted)
    })
}

/// Whether `line` is a `#[test]` (or `#[tokio::test]`) attribute.
pub(crate) fn is_test_attribute(line: &str) -> bool {
    line == "#[test]" || line.starts_with("#[tokio::test")
}

/// Whether `source` declares `name` as a `#[test]` (or `#[tokio::test]`)
/// function.
///
/// The attribute is required, so a name mentioned in a doc comment, a string
/// literal, or a helper function is not mistaken for a fixture. Attribute,
/// comment, and blank lines between the attribute and the signature are
/// skipped, because `#[ignore = "…"]` and a doc comment routinely sit there.
pub(crate) fn declares_test(source: &str, name: &str) -> bool {
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
pub(crate) fn declared_test_names(source: &str, prefix: &str) -> Vec<String> {
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
