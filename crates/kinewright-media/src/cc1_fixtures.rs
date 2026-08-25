//! Objective CC1 evidence fixtures.
//!
//! These tests intentionally live inside the media crate rather than in an
//! external integration-test crate.  The managed decoder and `Rgba16Float`
//! working frame are internal seams, and keeping the evidence next to them
//! lets the fixtures exercise the actual FFmpeg/swscale and production GPU
//! paths without making runtime implementation details part of the public
//! media API.

#![allow(clippy::cast_precision_loss)]
#![allow(clippy::cast_possible_truncation)]
#![allow(clippy::cast_sign_loss)]
#![allow(clippy::collapsible_if)]
#![allow(clippy::float_cmp)]
#![allow(clippy::format_push_string)]
#![allow(clippy::ignored_unit_patterns)]
#![allow(clippy::manual_div_ceil)]
#![allow(clippy::manual_let_else)]
#![allow(clippy::map_unwrap_or)]
#![allow(clippy::needless_pass_by_value)]
#![allow(clippy::too_many_arguments)]
#![allow(clippy::too_many_lines)]
#![allow(clippy::uninlined_format_args)]

use std::{
    collections::BTreeMap,
    io::Write,
    path::{Path, PathBuf},
    process::{Command as ProcessCommand, Stdio},
    sync::Arc,
    time::Instant,
};

use half::f16;
use kinewright_core::{
    Analysis, AssetId, Clip, ClipContent, ClipId, ColorBitDepth, ColorContext, ColorDescription,
    ColorMatrix, ColorPrimaries, ColorProvenance, ColorRange, ColorSourceProfile,
    ColorSourceProfileAssumption, ColorTransfer, ColorWhitePoint, Command, Core, DeliveryProfile,
    Document, Effect, EffectId, Event, ExportCancellation, ExportSettings, JournalCommand,
    MediaAsset, MediaError, MediaKind, Operation, ParamValue, Rational, TimeCode, Track, TrackId,
    TrackKind, delivery_conformance, effect_compatibility_stage, is_legacy_display_effect,
};
use serde_json::{Value, json};

use crate::{
    Compositor, CompositorLayer, GpuContext,
    color_pipeline::{
        DELIVERY_INTERMEDIATE_WHITE, PrimaryCorrection, PrimaryParameter,
        apply_primary_corrections, classify_source, classify_source_with_assumption, decode_bt709,
        decode_srgb, encode_monitor_rgb8, encode_monitor_rgba8, expand_native_range,
    },
    decode::{VideoDecoder, probe_path},
    frame::WorkingFrame,
    gpu_test_support::HARDWARE_GPU_OPT_IN_ENV,
    initialize_ffmpeg,
    sha256::Sha256,
    test_support::{TempDirectory, ffmpeg_executable},
    timeline::TransitionRenderParams,
};

pub(crate) const MONITOR_CPU_GPU_MAX: u8 = 2;
pub(crate) const MONITOR_CPU_GPU_P99: f64 = 1.0;
pub(crate) const MONITOR_CPU_GPU_MEAN: f64 = 0.50;
pub(crate) const LINEAR_CPU_GPU_MAX: f32 = 1.5e-3;
pub(crate) const LINEAR_CPU_GPU_P99: f32 = 7.5e-4;
pub(crate) const LINEAR_CPU_GPU_MEAN: f32 = 2.5e-4;
const DELIVERY_CODEC_MAX: u8 = 4;
const DELIVERY_CODEC_P99: f64 = 2.0;
const DELIVERY_CODEC_MEAN: f64 = 1.0;

/// §6.2 splits the linear comparison domain: it is defined on finite samples
/// with `|linear| <= 2` and calls its numbers "roughly one to two ULPs around
/// unity".  One `Rgba16Float` ULP is `2^-11` just below 1.0 but `2^-10`
/// (9.765625e-4) just above it, so the §6.2 P99 of 7.5e-4 is *tighter than the
/// normative storage format* for a sample in `[1, 2)`.  The fixture therefore
/// applies §6.2 verbatim on the in-gamut band and keeps the §6.2 maximum plus
/// an explicit one-ULP P99/mean on the over-range band, instead of quietly
/// widening the gate everywhere or dropping over-range coverage.
pub(crate) const LINEAR_GATE_IN_GAMUT: f32 = 1.0;
pub(crate) const LINEAR_GATE_DOMAIN: f32 = 2.0;
pub(crate) const LINEAR_OVER_RANGE_P99: f32 = 9.765_625e-4;
pub(crate) const LINEAR_OVER_RANGE_MEAN: f32 = 9.765_625e-4;

/// §6.2 neutral-identity monitor gate, used by the decoded identity ramps.
pub(crate) const IDENTITY_RAMP_MONITOR_MAX: u8 = 1;
pub(crate) const IDENTITY_RAMP_MONITOR_P99: f64 = 1.0;
pub(crate) const IDENTITY_RAMP_MONITOR_MEAN: f64 = 0.25;

/// Linear-working gate for the decoded identity ramps.
///
/// This is deliberately a *different* comparison from the CPU/GPU parity gate.
/// It measures the explicitly configured swscale `RGBA64` boundary against the
/// §3.1 native-code reference equations, so the error budget is swscale's
/// fixed-point rounding rather than `Rgba16Float` storage quantization. The
/// numbers are set to the §6.2 linear gate because the observed error on the
/// pinned `FFmpeg` build is roughly an order of magnitude smaller; the fixture
/// asserts max, `P99`, and mean so a regression cannot hide behind one loose
/// maximum.
const IDENTITY_RAMP_SWSCALE_MAX: f32 = 1.5e-3;
const IDENTITY_RAMP_SWSCALE_P99: f32 = 7.5e-4;
const IDENTITY_RAMP_SWSCALE_MEAN: f32 = 2.5e-4;

/// Two serial primary nodes must recover an over-range value through the
/// production `Rgba16Float` working surface.  This is a half-float round-trip
/// bound for one deterministic recovery raster, not the statistical CPU/GPU
/// parity gate of §6.2, so it gets its own name.
const NO_CLAMP_RECOVERY_MAX: f32 = 1.5e-3;

/// A non-neutral single-control case must visibly move the fixture raster in
/// the linear working domain.
///
/// Expressed in basis points of the compared RGB samples so the guard scales
/// with the raster.  A control case whose CPU reference is identical to the
/// neutral reference is a provable no-op and must fail loudly rather than
/// report a flattering `linear_max_error: 0.0`.
pub(crate) const MIN_CHANGED_LINEAR_BASIS_POINTS: u64 = 500;

/// Absolute/relative tolerance used when the f32 production reference is
/// compared against the f64 spec equations written out in this file.
const SPEC_F64_TOLERANCE: f64 = 1.0e-6;

#[derive(Debug, Clone, Copy)]
enum RampEncoding {
    Rec709,
    Srgb,
}

#[derive(Debug, Clone)]
struct RampSpec {
    name: &'static str,
    profile: ColorSourceProfile,
    depth: u8,
    range: ColorRange,
    encoding: RampEncoding,
    pixel_format: &'static str,
}

#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct DiffMetrics {
    pub(crate) max: u8,
    pub(crate) p99: f64,
    pub(crate) mean: f64,
}

#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct FloatDiffMetrics {
    pub(crate) max: f32,
    pub(crate) p99: f32,
    pub(crate) mean: f32,
}

/// The linear CPU/GPU comparison split by §6.2 magnitude band.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct LinearParityMetrics {
    /// `|reference| <= 1.0`: the §6.2 gate applies verbatim.
    pub(crate) in_gamut: FloatDiffMetrics,
    pub(crate) in_gamut_samples: usize,
    /// `1.0 < |reference| <= 2.0`: §6.2 maximum plus the one-ULP P99/mean.
    pub(crate) over_range: FloatDiffMetrics,
    pub(crate) over_range_samples: usize,
    /// `|reference| > 2.0`, excluded from the linear gate per §6.2.
    ///
    /// This is a legitimate exclusion: §6.2 defines the gate only over the
    /// bounded domain, so a reference beyond it has no stated tolerance.
    pub(crate) above_domain: usize,
    /// A NaN or infinity on either side.
    ///
    /// This is NOT a legitimate exclusion. Folding it into `above_domain`
    /// would let a GPU that produced NaN quietly drop those samples out of the
    /// gate and still report parity, which is exactly the failure the CC1
    /// finiteness claim exists to catch. It is counted separately and asserted
    /// to be zero.
    pub(crate) non_finite: usize,
}

impl LinearParityMetrics {
    pub(crate) const fn compared(&self) -> usize {
        self.in_gamut_samples + self.over_range_samples
    }

    pub(crate) fn as_json(&self) -> Value {
        json!({
            "in_gamut_max_error": self.in_gamut.max,
            "in_gamut_p99_error": self.in_gamut.p99,
            "in_gamut_mean_error": self.in_gamut.mean,
            "in_gamut_rgb_samples": self.in_gamut_samples,
            "over_range_max_error": self.over_range.max,
            "over_range_p99_error": self.over_range.p99,
            "over_range_mean_error": self.over_range.mean,
            "over_range_rgb_samples": self.over_range_samples,
            "above_domain_rgb_samples": self.above_domain,
            "non_finite_rgb_samples": self.non_finite,
        })
    }
}

fn rec709_description(depth: u8, range: ColorRange, transfer: ColorTransfer) -> ColorDescription {
    ColorDescription {
        primaries: ColorPrimaries::Bt709,
        transfer,
        matrix: ColorMatrix::Bt709,
        range,
        white_point: ColorWhitePoint::D65,
        bit_depth: ColorBitDepth::Integer(u16::from(depth)),
        confidence_basis_points: 10_000,
        provenance: ColorProvenance::UserOverride,
    }
}

fn srgb_description(depth: u8) -> ColorDescription {
    ColorDescription {
        primaries: ColorPrimaries::Srgb,
        transfer: ColorTransfer::Srgb,
        matrix: ColorMatrix::Identity,
        range: ColorRange::Full,
        white_point: ColorWhitePoint::D65,
        bit_depth: ColorBitDepth::Integer(u16::from(depth)),
        confidence_basis_points: 10_000,
        provenance: ColorProvenance::UserOverride,
    }
}

fn cc0_application_description(matrix: ColorMatrix, range: ColorRange) -> ColorDescription {
    ColorDescription {
        primaries: ColorPrimaries::Bt709,
        transfer: ColorTransfer::Bt709,
        matrix,
        range,
        white_point: ColorWhitePoint::D65,
        bit_depth: ColorBitDepth::Eight,
        confidence_basis_points: 10_000,
        provenance: ColorProvenance::ApplicationDefault,
    }
}

fn ramp_specs() -> [RampSpec; 5] {
    [
        RampSpec {
            name: "rec709_full_8",
            profile: ColorSourceProfile::Rec709Video,
            depth: 8,
            range: ColorRange::Full,
            encoding: RampEncoding::Rec709,
            pixel_format: "yuv444p",
        },
        RampSpec {
            name: "rec709_limited_8",
            profile: ColorSourceProfile::Rec709Video,
            depth: 8,
            range: ColorRange::Limited,
            encoding: RampEncoding::Rec709,
            pixel_format: "yuv444p",
        },
        RampSpec {
            name: "rec709_full_10",
            profile: ColorSourceProfile::Rec709Video,
            depth: 10,
            range: ColorRange::Full,
            encoding: RampEncoding::Rec709,
            pixel_format: "yuv444p10le",
        },
        RampSpec {
            name: "rec709_limited_10",
            profile: ColorSourceProfile::Rec709Video,
            depth: 10,
            range: ColorRange::Limited,
            encoding: RampEncoding::Rec709,
            pixel_format: "yuv444p10le",
        },
        RampSpec {
            name: "srgb_full_8",
            profile: ColorSourceProfile::SrgbFull,
            depth: 8,
            range: ColorRange::Full,
            encoding: RampEncoding::Srgb,
            pixel_format: "gbrp",
        },
    ]
}

pub(crate) fn output_hash(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    let mut result = String::with_capacity(64);
    for byte in hasher.finalize() {
        result.push_str(&format!("{byte:02x}"));
    }
    result
}

pub(crate) fn file_hash(path: &Path) -> String {
    let bytes = std::fs::read(path)
        .unwrap_or_else(|error| panic!("could not hash fixture file {}: {error}", path.display()));
    output_hash(&bytes)
}

pub(crate) fn git_revision() -> String {
    let revision = ProcessCommand::new("git")
        .args(["rev-parse", "HEAD"])
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_owned())
        .filter(|revision| !revision.is_empty());
    let Some(revision) = revision else {
        return "unavailable".to_owned();
    };
    // Evidence must not claim a clean revision for code that is not committed.
    let dirty = ProcessCommand::new("git")
        .args(["status", "--porcelain", "--untracked-files=no"])
        .output()
        .ok()
        .filter(|output| output.status.success())
        .is_some_and(|output| !output.stdout.iter().all(u8::is_ascii_whitespace));
    if dirty {
        format!("{revision}-dirty")
    } else {
        revision
    }
}

fn emit_evidence(
    fixture: &str,
    backend: &str,
    profile: Option<ColorSourceProfile>,
    depth: Option<u8>,
    controls: Value,
    raster: (u32, u32),
    source_hash: Option<String>,
    output_hash: String,
    metrics: Value,
) {
    let backend_provenance = backend_metadata(backend);
    let backend_name = backend_provenance
        .get("backend")
        .cloned()
        .unwrap_or(Value::Null);
    let adapter = backend_provenance
        .get("adapter")
        .cloned()
        .unwrap_or(Value::Null);
    let software_fallback = backend_provenance
        .get("software_fallback")
        .cloned()
        .unwrap_or(Value::Null);
    let gpu_claim = backend_provenance
        .get("gpu_claim")
        .cloned()
        .unwrap_or(Value::Null);
    let lane = backend_provenance
        .get("lane")
        .cloned()
        .unwrap_or(Value::Null);
    let payload = json!({
        "contract": "cc1_managed_sdr_primary",
        "fixture": fixture,
        "git_revision": git_revision(),
        "backend": backend,
        "backend_name": backend_name,
        "adapter": adapter,
        "software_fallback": software_fallback,
        "gpu_claim": gpu_claim,
        "lane": lane,
        "backend_metadata": backend_provenance,
        "os": std::env::consts::OS,
        "arch": std::env::consts::ARCH,
        "source_profile": profile.map(ColorSourceProfile::id),
        "source_depth_bits": depth,
        "context": ColorContext::sdr_rec709(),
        "controls": controls,
        "raster": {"width": raster.0, "height": raster.1},
        "source_hash_sha256": source_hash,
        "output_hash_sha256": output_hash,
        "metrics": metrics,
    });
    println!("CC1_EVIDENCE {payload}");
    write_evidence_artefact(fixture, &payload);
}

/// Persist one evidence payload under `target/color-evidence/`.
///
/// `emit_evidence` only reaches a human under `--nocapture`, which makes an
/// audit depend on how the suite happened to be invoked.  Writing the same
/// JSON to a file leaves an artefact behind for every run.  Nothing asserts on
/// these files: a read-only or full filesystem must not fail a colour fixture,
/// so a write failure is reported and ignored.
pub(crate) fn write_evidence_artefact(fixture: &str, payload: &Value) {
    let directory = evidence_directory();
    if let Err(error) = std::fs::create_dir_all(&directory) {
        eprintln!(
            "CC1_EVIDENCE_ARTEFACT could not create {}: {error}",
            directory.display()
        );
        return;
    }
    let sanitized = fixture
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || character == '-' || character == '_' {
                character
            } else {
                '_'
            }
        })
        .collect::<String>();
    let path = directory.join(format!("{sanitized}.json"));
    if let Err(error) = std::fs::write(&path, format!("{payload:#}\n")) {
        eprintln!(
            "CC1_EVIDENCE_ARTEFACT could not write {}: {error}",
            path.display()
        );
    }
}

fn evidence_directory() -> PathBuf {
    if let Some(target) = std::env::var_os("CARGO_TARGET_DIR") {
        return PathBuf::from(target).join("color-evidence");
    }
    // `CARGO_MANIFEST_DIR` is `crates/kinewright-media`; the workspace target
    // directory is two levels up.
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/color-evidence")
}

pub(crate) fn backend_metadata(backend: &str) -> Value {
    let mut metadata = serde_json::Map::new();
    for token in backend.split(';') {
        let Some((key, value)) = token.split_once('=') else {
            continue;
        };
        match key {
            "backend" | "adapter" | "lane" => {
                metadata.insert(key.to_owned(), Value::String(value.to_owned()));
            }
            "software_fallback" | "gpu_claim" => {
                if let Ok(value) = value.parse::<bool>() {
                    metadata.insert(key.to_owned(), Value::Bool(value));
                }
            }
            _ => {}
        }
    }
    Value::Object(metadata)
}

fn source_description_for(spec: &RampSpec) -> ColorDescription {
    match spec.encoding {
        RampEncoding::Rec709 => {
            rec709_description(spec.depth, spec.range.clone(), ColorTransfer::Bt709)
        }
        RampEncoding::Srgb => srgb_description(spec.depth),
    }
}

fn write_u16_le(destination: &mut Vec<u8>, value: u16) {
    destination.extend_from_slice(&value.to_le_bytes());
}

fn ramp_native_code(spec: &RampSpec, position: u32) -> u32 {
    let max = (1_u32 << spec.depth) - 1;
    match &spec.range {
        ColorRange::Limited => {
            // Keep a limited-range fixture inside the legal luma interval.
            // Feeding 0..max while tagging the stream as TV range creates
            // intentionally out-of-range RGB clipping and makes neutrality
            // depend on swscale's negative-value rounding.
            let low = 1_u32 << (spec.depth - 4);
            let high = 235_u32 << (spec.depth - 8);
            low + ((high - low) * position + max / 2) / max
        }
        _ => position,
    }
}

fn raw_ramp_bytes(spec: &RampSpec) -> (u32, Vec<u8>) {
    let max = (1_u32 << spec.depth) - 1;
    let width = max + 1;
    let mut bytes = Vec::new();
    match spec.encoding {
        RampEncoding::Rec709 => {
            let bytes_per_sample = usize::from(spec.depth > 8) + 1;
            for plane in 0..3 {
                for code in 0..=max {
                    let sample = if plane == 0 {
                        ramp_native_code(spec, code)
                    } else {
                        (max + 1) / 2
                    };
                    if bytes_per_sample == 1 {
                        bytes.push(u8::try_from(sample).expect("8-bit ramp code"));
                    } else {
                        write_u16_le(&mut bytes, u16::try_from(sample).expect("10-bit ramp code"));
                    }
                }
            }
        }
        RampEncoding::Srgb => {
            // `gbrp` is planar. A gray ramp is deliberately used so channel
            // order cannot mask a source/profile or transfer error.
            for _plane in 0..3 {
                for code in 0..=max {
                    bytes.push(u8::try_from(code).expect("8-bit sRGB ramp code"));
                }
            }
        }
    }
    (width, bytes)
}

fn verify_native_ramp(path: &Path, spec: &RampSpec, width: u32, expected: &[u8]) {
    let output = ProcessCommand::new(ffmpeg_executable())
        .args(["-hide_banner", "-loglevel", "error", "-i"])
        .arg(path)
        .args([
            "-frames:v",
            "1",
            "-f",
            "rawvideo",
            "-pix_fmt",
            spec.pixel_format,
            "pipe:1",
        ])
        .output()
        .unwrap_or_else(|error| {
            panic!("native {} ramp decode failed to start: {error}", spec.name)
        });
    assert!(
        output.status.success(),
        "native {} ramp decode failed: {}",
        spec.name,
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        output.stdout.len(),
        expected.len(),
        "native {} ramp byte length changed for {width} samples",
        spec.name
    );
    assert_eq!(
        output.stdout, expected,
        "native {} ramp did not round-trip through FFV1",
        spec.name
    );
}

fn generate_ramp_media(directory: &TempDirectory, spec: &RampSpec) -> (PathBuf, u32, Vec<u8>) {
    let (width, input) = raw_ramp_bytes(spec);
    // FFmpeg selects the muxer from the output suffix.  Keep these as real
    // Matroska fixtures so the generated media is decoded through the same
    // probe/managed-decoder path as project sources.
    let path = directory.path(&format!("{}.mkv", spec.name));
    let size = format!("{width}x1");
    let color_range = match &spec.range {
        ColorRange::Full => "pc",
        ColorRange::Limited => "tv",
        _ => unreachable!("ramp specs use full or limited range"),
    };
    let (color_transfer, color_matrix, color_primaries) = match spec.encoding {
        RampEncoding::Rec709 => ("bt709", "bt709", "bt709"),
        RampEncoding::Srgb => ("iec61966-2-1", "rgb", "bt709"),
    };
    let setparams = format!(
        "setparams=range={}:color_primaries={color_primaries}:color_trc={color_transfer}:colorspace={setparams_matrix}",
        if matches!(&spec.range, ColorRange::Limited) {
            "limited"
        } else {
            "full"
        },
        setparams_matrix = if matches!(spec.encoding, RampEncoding::Srgb) {
            "gbr"
        } else {
            color_matrix
        },
    );
    let mut command = ProcessCommand::new(ffmpeg_executable());
    command
        .args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-y",
            "-f",
            "rawvideo",
            "-pix_fmt",
            spec.pixel_format,
            "-s",
            &size,
            "-r",
            "1",
            "-i",
            "pipe:0",
            "-vf",
            &setparams,
            "-frames:v",
            "1",
            "-an",
            "-c:v",
            "ffv1",
            "-level",
            "3",
            "-g",
            "1",
            "-pix_fmt",
            spec.pixel_format,
            "-color_primaries",
            color_primaries,
            "-color_trc",
            color_transfer,
            "-colorspace",
            color_matrix,
            "-color_range",
            color_range,
        ])
        .arg(&path)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    let mut child = command
        .spawn()
        .unwrap_or_else(|error| panic!("could not start FFmpeg for {}: {error}", spec.name));
    child
        .stdin
        .take()
        .expect("FFmpeg stdin should be piped")
        .write_all(&input)
        .unwrap_or_else(|error| panic!("could not write {} ramp bytes: {error}", spec.name));
    let output = child
        .wait_with_output()
        .unwrap_or_else(|error| panic!("FFmpeg {} ramp process failed: {error}", spec.name));
    assert!(
        output.status.success(),
        "FFmpeg {} ramp generation failed: {}",
        spec.name,
        String::from_utf8_lossy(&output.stderr)
    );
    verify_native_ramp(&path, spec, width, &input);
    (path, width, input)
}

fn decode_actual_ramp(
    path: &Path,
    width: u32,
    description: &ColorDescription,
) -> (MediaAsset, WorkingFrame) {
    let asset = probe_path(path, AssetId(1)).expect("ramp should probe");
    let expected_profile = classify_source(description).expect("ramp description should classify");
    assert_eq!(
        classify_source_with_assumption(
            &asset.color_description,
            Some(ColorSourceProfileAssumption::D65),
        ),
        Ok(expected_profile),
        "actual probed ramp metadata must classify as the fixture profile"
    );
    assert_eq!(
        asset.color_description.transfer, description.transfer,
        "actual probed ramp transfer must match the tagged source"
    );
    assert_eq!(
        asset.color_description.range, description.range,
        "actual probed ramp range must match the tagged source"
    );
    let assumption = Some(ColorSourceProfileAssumption::D65);
    let mut decoder = VideoDecoder::open_scaled_managed(
        path,
        Rational::new(1, 1).expect("one fps"),
        None,
        description,
        assumption,
    )
    .unwrap_or_else(|error| panic!("managed ramp decode failed for {}: {error}", path.display()));
    let mut cache = crate::cache::FrameCache::<WorkingFrame>::new(2);
    decoder
        .decode_window(TimeCode::ZERO, TimeCode::ZERO, &mut cache)
        .unwrap_or_else(|error| panic!("managed ramp frame decode failed: {error}"));
    let frame = cache
        .frame_at_or_before(TimeCode::ZERO)
        .expect("managed ramp frame should be cached");
    assert_eq!(frame.width, width);
    assert_eq!(frame.height, 1);
    (asset, frame)
}

fn monitor_ramp(frame: &WorkingFrame) -> Vec<u8> {
    frame
        .pixels
        .as_chunks::<4>()
        .0
        .iter()
        .flat_map(|rgba| {
            encode_monitor_rgba8([
                rgba[0].to_f32(),
                rgba[1].to_f32(),
                rgba[2].to_f32(),
                rgba[3].to_f32(),
            ])
        })
        .collect()
}

fn expected_ramp_monitor(spec: &RampSpec) -> Vec<u8> {
    let (width, _) = raw_ramp_bytes(spec);
    let max = (1_u32 << spec.depth) - 1;
    (0..width)
        .flat_map(|code| {
            let encoded = ramp_native_code(spec, code) as f32 / max as f32;
            let coded_rgb = if matches!(&spec.range, ColorRange::Limited) {
                expand_native_range(
                    [encoded; 3],
                    &ColorBitDepth::Integer(u16::from(spec.depth)),
                    &ColorRange::Limited,
                )
                .expect("limited ramp expansion")
            } else {
                [encoded; 3]
            };
            let linear = match spec.encoding {
                RampEncoding::Rec709 => coded_rgb.map(decode_bt709),
                RampEncoding::Srgb => coded_rgb.map(decode_srgb),
            };
            encode_monitor_rgb8(linear)
                .into_iter()
                .chain(std::iter::once(255))
        })
        .collect()
}

fn assert_monotonic_ramp(rgba: &[u8], width: u32) {
    let pixels = rgba.as_chunks::<4>().0;
    assert_eq!(pixels.len(), usize::try_from(width).expect("ramp width"));
    for channel in 0..3 {
        for pair in pixels.windows(2) {
            assert!(
                pair[1][channel] >= pair[0][channel],
                "ramp descended at channel {channel}: {} -> {}",
                pair[0][channel],
                pair[1][channel]
            );
        }
    }
}

fn assert_neutral_pixels(rgba: &[u8], fixture: &str) {
    for (index, pixel) in rgba.as_chunks::<4>().0.iter().enumerate() {
        let spread = pixel[0]
            .max(pixel[1])
            .max(pixel[2])
            .saturating_sub(pixel[0].min(pixel[1]).min(pixel[2]));
        assert!(
            spread <= 1,
            "{fixture} neutral ramp lost neutrality at pixel {index}: {pixel:?}"
        );
    }
}

pub(crate) fn abs_code_diff_rgb(actual: &[u8], expected: &[u8]) -> DiffMetrics {
    assert_eq!(actual.len(), expected.len());
    let mut differences = actual
        .as_chunks::<4>()
        .0
        .iter()
        .zip(expected.as_chunks::<4>().0.iter())
        .flat_map(|(actual, expected)| {
            assert_eq!(actual[3], expected[3], "GPU/source alpha must remain exact");
            actual[..3]
                .iter()
                .zip(&expected[..3])
                .map(|(actual, expected)| f64::from(actual.abs_diff(*expected)))
                .collect::<Vec<_>>()
        })
        .collect::<Vec<_>>();
    differences.sort_by(f64::total_cmp);
    let max = differences.last().copied().unwrap_or(0.0) as u8;
    let p99_index = ((differences.len() as f64 * 0.99).ceil() as usize)
        .saturating_sub(1)
        .min(differences.len().saturating_sub(1));
    let p99 = differences.get(p99_index).copied().unwrap_or(0.0);
    let mean = if differences.is_empty() {
        0.0
    } else {
        differences.iter().sum::<f64>() / differences.len() as f64
    };
    DiffMetrics { max, p99, mean }
}

pub(crate) fn monitor_luma_and_clipping(rgba: &[u8]) -> Value {
    let mut lumas = Vec::with_capacity(rgba.len() / 4);
    let mut clipped_channels = 0_u64;
    for pixel in rgba.as_chunks::<4>().0 {
        lumas.push(
            0.2126 * f64::from(pixel[0])
                + 0.7152 * f64::from(pixel[1])
                + 0.0722 * f64::from(pixel[2]),
        );
        clipped_channels += pixel[..3]
            .iter()
            .filter(|value| **value == 0 || **value == u8::MAX)
            .count() as u64;
    }
    let first_luma = lumas.first().copied().unwrap_or(0.0);
    lumas.sort_by(f64::total_cmp);
    let percentile = |fraction: f64| {
        let index = ((lumas.len() as f64 * fraction).ceil() as usize)
            .saturating_sub(1)
            .min(lumas.len().saturating_sub(1));
        lumas.get(index).copied().unwrap_or(0.0)
    };
    let channel_count = u64::try_from(rgba.len() / 4 * 3).unwrap_or(0);
    let clipping_basis_points = if channel_count == 0 {
        0.0
    } else {
        clipped_channels as f64 * 10_000.0 / channel_count as f64
    };
    json!({
        "first_luma": first_luma,
        "median_luma": percentile(0.50),
        "p99_luma": percentile(0.99),
        "clipping_basis_points": clipping_basis_points,
    })
}

pub(crate) fn abs_float_diff(actual: &[f32], expected: &[f32]) -> FloatDiffMetrics {
    assert_eq!(actual.len(), expected.len());
    let mut differences = actual
        .iter()
        .zip(expected)
        .map(|(actual, expected)| (actual - expected).abs())
        .collect::<Vec<_>>();
    differences.sort_by(f32::total_cmp);
    let max = differences.last().copied().unwrap_or(0.0);
    let p99_index = ((differences.len() as f32 * 0.99).ceil() as usize)
        .saturating_sub(1)
        .min(differences.len().saturating_sub(1));
    let p99 = differences.get(p99_index).copied().unwrap_or(0.0);
    let mean = if differences.is_empty() {
        0.0
    } else {
        differences.iter().sum::<f32>() / differences.len() as f32
    };
    FloatDiffMetrics { max, p99, mean }
}

/// Split the linear CPU/GPU comparison into the §6.2 magnitude bands.
///
/// Band membership is decided by the CPU reference, which is the comparison
/// source named by §6.2, so a GPU value that has drifted out of the domain
/// cannot exclude itself from the gate.
pub(crate) fn linear_parity_metrics(actual: &[f32], expected: &[f32]) -> LinearParityMetrics {
    assert_eq!(actual.len(), expected.len());
    let mut in_gamut = (Vec::new(), Vec::new());
    let mut over_range = (Vec::new(), Vec::new());
    let mut above_domain = 0;
    let mut non_finite = 0;
    for (actual_pixel, expected_pixel) in actual
        .as_chunks::<4>()
        .0
        .iter()
        .zip(expected.as_chunks::<4>().0.iter())
    {
        assert!(
            (actual_pixel[3] - expected_pixel[3]).abs() <= 1.0e-6,
            "production alpha changed: actual={} expected={}",
            actual_pixel[3],
            expected_pixel[3]
        );
        for channel in 0..3 {
            let actual_value = actual_pixel[channel];
            let expected_value = expected_pixel[channel];
            let magnitude = expected_value.abs();
            if !actual_value.is_finite() || !expected_value.is_finite() {
                non_finite += 1;
            } else if magnitude <= LINEAR_GATE_IN_GAMUT {
                in_gamut.0.push(actual_value);
                in_gamut.1.push(expected_value);
            } else if magnitude <= LINEAR_GATE_DOMAIN {
                over_range.0.push(actual_value);
                over_range.1.push(expected_value);
            } else {
                above_domain += 1;
            }
        }
    }
    LinearParityMetrics {
        in_gamut: abs_float_diff(&in_gamut.0, &in_gamut.1),
        in_gamut_samples: in_gamut.0.len(),
        over_range: abs_float_diff(&over_range.0, &over_range.1),
        over_range_samples: over_range.0.len(),
        above_domain,
        non_finite,
    }
}

/// Apply the §6.2 linear gate, band by band.
pub(crate) fn assert_linear_parity(metrics: &LinearParityMetrics, label: &str) {
    // The CC1 doc claims the managed path never emits a non-finite sample.
    // Excluding one from the gate instead of failing would let a NaN-producing
    // GPU report parity, so this is a hard failure rather than a band.
    assert_eq!(
        metrics.non_finite, 0,
        "non-finite linear sample for {label}: {metrics:?}"
    );
    assert!(
        metrics.in_gamut.max <= LINEAR_CPU_GPU_MAX,
        "GPU/CPU in-gamut linear max for {label}: {metrics:?}"
    );
    assert!(
        metrics.in_gamut.p99 <= LINEAR_CPU_GPU_P99,
        "GPU/CPU in-gamut linear P99 for {label}: {metrics:?}"
    );
    assert!(
        metrics.in_gamut.mean <= LINEAR_CPU_GPU_MEAN,
        "GPU/CPU in-gamut linear mean for {label}: {metrics:?}"
    );
    assert!(
        metrics.over_range.max <= LINEAR_CPU_GPU_MAX,
        "GPU/CPU over-range linear max for {label}: {metrics:?}"
    );
    assert!(
        metrics.over_range.p99 <= LINEAR_OVER_RANGE_P99,
        "GPU/CPU over-range linear P99 for {label}: {metrics:?}"
    );
    assert!(
        metrics.over_range.mean <= LINEAR_OVER_RANGE_MEAN,
        "GPU/CPU over-range linear mean for {label}: {metrics:?}"
    );
}

fn effect_with_parameters(
    id: u64,
    parameters: impl IntoIterator<Item = (&'static str, i64)>,
) -> Effect {
    Effect {
        id: EffectId(id),
        name: "primary_correction".to_owned(),
        parameters: parameters
            .into_iter()
            .map(|(name, value)| (name.to_owned(), ParamValue::Integer(value)))
            .collect(),
        keyframes: BTreeMap::new(),
    }
}

fn correction_effect(id: u64, correction: PrimaryCorrection) -> Effect {
    effect_with_parameters(
        id,
        PrimaryParameter::ALL
            .into_iter()
            .map(|parameter| (parameter.name(), correction.parameter(parameter))),
    )
}

fn representative_correction() -> PrimaryCorrection {
    PrimaryCorrection {
        exposure_milli_stops: 1_000,
        temperature_percent: 35,
        tint_percent: -30,
        contrast_percent: 25,
        contrast_pivot_basis_points: 4_200,
        blacks_percent: -35,
        shadows_percent: 25,
        highlights_percent: -20,
        whites_percent: 30,
        saturation_percent: 40,
    }
}

fn correction_value_json(correction: PrimaryCorrection) -> Value {
    let values = PrimaryParameter::ALL
        .into_iter()
        .map(|parameter| {
            (
                parameter.name().to_owned(),
                Value::from(correction.parameter(parameter)),
            )
        })
        .collect::<serde_json::Map<_, _>>();
    Value::Object(values)
}

fn chart_patches() -> [(&'static str, [f32; 3]); 12] {
    [
        ("black", [0.0, 0.0, 0.0]),
        ("near_black", [0.01, 0.01, 0.01]),
        ("gray_18", [0.18, 0.18, 0.18]),
        ("mid_gray", [0.5, 0.5, 0.5]),
        ("near_white", [0.9, 0.9, 0.9]),
        ("white", [1.0, 1.0, 1.0]),
        ("red", [1.0, 0.0, 0.0]),
        ("green", [0.0, 1.0, 0.0]),
        ("blue", [0.0, 0.0, 1.0]),
        ("cyan", [0.0, 1.0, 1.0]),
        ("magenta", [1.0, 0.0, 1.0]),
        ("yellow", [1.0, 1.0, 0.0]),
    ]
}

/// The §3.1 BT.709 monitoring OETF, written out in `f64` straight from the
/// specification text.
///
/// This is deliberately *not* a call into `color_pipeline::encode_bt709`: an
/// expectation computed by the implementation under test cannot detect a
/// change to that implementation.
fn spec_encode_bt709_f64(linear: f64) -> f64 {
    if linear.abs() < 0.018 {
        4.5 * linear
    } else {
        linear.signum() * (1.099 * linear.abs().powf(0.45) - 0.099)
    }
}

/// §2.2.5: the only RGB clamp is the final monitor quantization.
fn spec_monitor_code_f64(linear: f64) -> u8 {
    (spec_encode_bt709_f64(linear).clamp(0.0, 1.0) * 255.0).round() as u8
}

fn spec_luma_f64(rgb: [f64; 3]) -> f64 {
    0.2126 * rgb[0] + 0.7152 * rgb[1] + 0.0722 * rgb[2]
}

fn spec_smoothstep_f64(start: f64, end: f64, value: f64) -> f64 {
    let t = ((value - start) / (end - start)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// The complete §3.2 control chain in `f64`, in the canonical order: white
/// balance, exposure, tonal balance, contrast around the pivot, saturation
/// around Rec.709 luma.
fn spec_apply_primary_f64(correction: PrimaryCorrection, rgb: [f64; 3]) -> [f64; 3] {
    // 1. White balance: equal and opposite 10% red/blue gains for temperature,
    //    an opposite 10% green gain for tint, applied around unity.
    let temperature = f64::from(correction.temperature_percent) / 100.0;
    let tint = f64::from(correction.tint_percent) / 100.0;
    let gains = [
        1.0 + 0.1 * temperature,
        1.0 - 0.1 * tint,
        1.0 - 0.1 * temperature,
    ];
    // 2. Exposure: multiply linear RGB by 2^(value/1000).
    let exposure = 2.0_f64.powf(f64::from(correction.exposure_milli_stops) / 1_000.0);
    let mut value = [
        rgb[0] * gains[0] * exposure,
        rgb[1] * gains[1] * exposure,
        rgb[2] * gains[2] * exposure,
    ];
    // 3. Blacks/shadows/highlights/whites. The smoothstep weights use the
    //    clamped u, but x itself is never clamped.
    for channel in &mut value {
        let u = channel.clamp(0.0, 1.0);
        let black = 1.0 - spec_smoothstep_f64(0.00, 0.25, u);
        let shadow = 1.0 - spec_smoothstep_f64(0.15, 0.50, u);
        let highlight = spec_smoothstep_f64(0.50, 0.85, u);
        let white = spec_smoothstep_f64(0.75, 1.00, u);
        *channel += 0.25 * f64::from(correction.blacks_percent) / 100.0 * black;
        *channel += 0.20 * f64::from(correction.shadows_percent) / 100.0 * shadow;
        *channel += 0.20 * f64::from(correction.highlights_percent) / 100.0 * highlight;
        *channel += 0.25 * f64::from(correction.whites_percent) / 100.0 * white;
    }
    // 4. Contrast around contrast_pivot_basis_points.
    let pivot = f64::from(correction.contrast_pivot_basis_points) / 10_000.0;
    let contrast = 1.0 + f64::from(correction.contrast_percent) / 100.0;
    for channel in &mut value {
        *channel = pivot + (*channel - pivot) * contrast;
    }
    // 5. Saturation around Rec.709 luma.
    let luma = spec_luma_f64(value);
    let saturation = 1.0 + f64::from(correction.saturation_percent) / 100.0;
    value.map(|channel| luma + (channel - luma) * saturation)
}

/// Compare one f32 production value against the f64 spec reference.
///
/// The tolerance scales with magnitude because a single f32 step at the
/// exposure bound (2^5 gain) is already larger than a fixed 1e-6.
fn assert_matches_spec_f64(actual: f32, expected: f64, label: &str) {
    let tolerance = SPEC_F64_TOLERANCE * expected.abs().max(1.0);
    assert!(
        (f64::from(actual) - expected).abs() <= tolerance,
        "{label}: production f32 {actual} does not match the f64 spec value {expected} within {tolerance}"
    );
}

fn chart_monitor(correction: PrimaryCorrection) -> Vec<u8> {
    chart_patches()
        .into_iter()
        .flat_map(|(_, rgb)| {
            encode_monitor_rgb8(correction.apply_checked(rgb).expect("chart controls"))
        })
        .collect()
}

fn chart_neutral_spread(output: &[u8]) -> u8 {
    chart_patches()
        .into_iter()
        .enumerate()
        .filter(|(_, (_, rgb))| rgb[0] == rgb[1] && rgb[1] == rgb[2])
        .map(|(index, _)| {
            let pixel = &output[index * 3..index * 3 + 3];
            pixel[0]
                .max(pixel[1])
                .max(pixel[2])
                .saturating_sub(pixel[0].min(pixel[1]).min(pixel[2]))
        })
        .max()
        .unwrap_or(0)
}

pub(crate) fn working_frame(width: u32, height: u32, rgb: &[[f32; 3]]) -> WorkingFrame {
    assert_eq!(
        rgb.len(),
        usize::try_from(width * height).expect("working frame size")
    );
    let mut pixels = Vec::with_capacity(rgb.len() * 4);
    for value in rgb {
        pixels.extend(value.map(f16::from_f32));
        pixels.push(f16::from_f32(1.0));
    }
    WorkingFrame {
        width,
        height,
        pixels: Arc::new(pixels),
    }
}

fn cpu_reference_monitor(frame: &WorkingFrame, corrections: &[PrimaryCorrection]) -> Vec<u8> {
    frame
        .pixels
        .as_chunks::<4>()
        .0
        .iter()
        .flat_map(|rgba| {
            let corrected = apply_primary_corrections(
                [rgba[0].to_f32(), rgba[1].to_f32(), rgba[2].to_f32()],
                corrections,
            )
            .expect("representative primary controls");
            let quantized = corrected.map(|value| f16::from_f32(value).to_f32());
            encode_monitor_rgba8([quantized[0], quantized[1], quantized[2], rgba[3].to_f32()])
        })
        .collect()
}

/// A 1:1 raster of the twelve reference patches, one wide block per patch.
///
/// The blocks are eight pixels wide so the production linear sampler is
/// measured on texel interiors instead of on eleven interpolated patch seams.
fn chart_frame() -> (u32, u32, WorkingFrame) {
    const PATCH_WIDTH: u32 = 8;
    let patches = chart_patches();
    let width = PATCH_WIDTH * patches.len() as u32;
    let height = 2;
    let rgb = (0..width * height)
        .map(|index| patches[(index % width / PATCH_WIDTH) as usize].1)
        .collect::<Vec<_>>();
    (width, height, working_frame(width, height, &rgb))
}

fn representative_frame() -> (u32, u32, WorkingFrame) {
    // Use wide low-frequency colour bars so the production linear sampler is
    // measured on stable texel interiors. The separate 12-patch fixture owns
    // high-frequency chart boundaries; keeping those boundaries out of this
    // parity raster prevents interpolation edge pixels from dominating a
    // half-float P99 gate while retaining varied RGB/control coverage.
    //
    // The bars must span the whole working domain, not just the bottom fifth
    // of it. Without a bar at or above 0.9 the smoothstep highlight/white
    // weights of §3.2 are identically zero, which silently turns every
    // `highlights_percent`/`whites_percent` parity case into a proven no-op.
    // Likewise a bar above 1.0 exercises the contrast pivot and the
    // no-intermediate-clamp requirement, and a bar containing a negative
    // channel exercises the sign-preserving §3.1 monitor encoding.
    let width = 512;
    let height = 4;
    let bars = [
        [0.0, 0.0, 0.0],
        [0.05, 0.05, 0.05],
        [0.1, 0.05, 0.025],
        [0.2, 0.025, 0.12],
        // Near-white neutral: drives w_white and w_highlight.
        [0.92, 0.92, 0.92],
        // Over-range: must survive every stage without an intermediate clamp.
        [1.5, 0.8, 0.3],
        // Recoverable undershoot for the sign-preserving encode path.
        [-0.05, 0.02, 0.1],
        // Upper mid-tones straddling the 0.5 contrast pivot.
        [0.55, 0.62, 0.48],
    ];
    let bar_width = width / bars.len() as u32;
    let rgb = (0..width * height)
        .map(|index| bars[(index % width / bar_width) as usize])
        .collect::<Vec<_>>();
    (width, height, working_frame(width, height, &rgb))
}

/// Parameters for one single-control parity case.
///
/// `contrast_pivot_basis_points` only has an observable effect when contrast is
/// non-neutral, so the pivot sweep is paired with a fixed non-zero contrast.
/// Sweeping the pivot at `contrast_percent = 0` measures nothing.
fn control_case_parameters(parameter: PrimaryParameter, value: i64) -> Vec<(&'static str, i64)> {
    if parameter == PrimaryParameter::ContrastPivotBasisPoints {
        return vec![(parameter.name(), value), ("contrast_percent", 50)];
    }
    vec![(parameter.name(), value)]
}

/// The independent CPU reference in the linear working domain, including the
/// normative `Rgba16Float` storage quantization.
fn cpu_reference_linear(frame: &WorkingFrame, correction: PrimaryCorrection) -> Vec<f32> {
    frame
        .pixels
        .as_chunks::<4>()
        .0
        .iter()
        .flat_map(|rgba| {
            let output = correction
                .apply_checked([rgba[0].to_f32(), rgba[1].to_f32(), rgba[2].to_f32()])
                .expect("linear reference");
            output
                .into_iter()
                .map(|value| f16::from_f32(value).to_f32())
                .chain(std::iter::once(f16::from_f32(rgba[3].to_f32()).to_f32()))
        })
        .collect()
}

/// Fail a control case whose CPU reference is indistinguishable from neutral.
///
/// A vacuous case reports a flattering `linear_max_error: 0.0` while proving
/// nothing about the control, so it must fail loudly.  The comparison is made
/// in the linear working domain rather than on monitor codes: the working
/// surface is the CC1 contract domain, and a fully saturated chart patch can
/// clip to the same 8-bit code under a real control change.
fn assert_case_is_not_vacuous(expected: &[f32], neutral: &[f32], correction: PrimaryCorrection) {
    if correction == PrimaryCorrection::default() {
        return;
    }
    assert_eq!(expected.len(), neutral.len());
    let compared = u64::try_from(expected.len() / 4 * 3).unwrap_or(0);
    let changed = expected
        .as_chunks::<4>()
        .0
        .iter()
        .zip(neutral.as_chunks::<4>().0.iter())
        .map(|(actual, neutral)| {
            actual[..3]
                .iter()
                .zip(&neutral[..3])
                .filter(|(actual, neutral)| actual != neutral)
                .count() as u64
        })
        .sum::<u64>();
    assert!(
        changed * 10_000 >= compared * MIN_CHANGED_LINEAR_BASIS_POINTS,
        "control case {correction:?} changed only {changed} of {compared} linear working RGB samples against neutral; a non-neutral CC1 control must move the fixture raster or the parity case proves nothing"
    );
}

/// Whether one parity case is required to exercise the §6.2 over-range band.
///
/// The band is populated from the CPU reference, so a case with an empty band
/// silently skips the over-range tolerances while still reporting a green
/// gate. Every case therefore states which it is, and the statement is
/// asserted rather than inferred from what the run happened to produce.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OverRangeBand {
    /// The case must land samples in `1 < |reference| <= 2`.
    Required,
    /// The control provably takes the fixture's over-range samples out of the
    /// band; the reason travels with the evidence.
    ProvablyEmpty(&'static str),
}

impl OverRangeBand {
    fn as_json(self) -> Value {
        match self {
            Self::Required => json!({"expectation": "required"}),
            Self::ProvablyEmpty(reason) => {
                json!({"expectation": "provably_empty", "reason": reason})
            }
        }
    }
}

/// Why a swept control may legitimately leave the over-range band empty on the
/// representative fixture.
///
/// The fixture's only over-range sample is the 1.5 red of the
/// `[1.5, 0.8, 0.3]` bar (the 0.92 neutral bar and the 0.55/0.62/0.48 bar are
/// both in gamut), so the band empties exactly when a control moves that
/// sample out of `(1, 2]`. Anything else must populate it.
fn representative_over_range_expectation(correction: PrimaryCorrection) -> OverRangeBand {
    if correction.exposure_milli_stops <= -1_000 {
        return OverRangeBand::ProvablyEmpty(
            "a stop or more of negative exposure scales the 1.5 sample below 1.0",
        );
    }
    if correction.contrast_percent == -100 {
        return OverRangeBand::ProvablyEmpty(
            "contrast -100 collapses every sample onto the contrast pivot",
        );
    }
    if correction.saturation_percent == -100 {
        return OverRangeBand::ProvablyEmpty(
            "full desaturation replaces the 1.5 red with the bar's 0.913 luma",
        );
    }
    if correction.saturation_percent == 100 {
        return OverRangeBand::ProvablyEmpty(
            "doubling chroma pushes the 1.5 red past 2.0, above the §6.2 domain",
        );
    }
    OverRangeBand::Required
}

fn assert_gpu_control_case(
    compositor: &Compositor,
    width: u32,
    height: u32,
    frame: &WorkingFrame,
    correction: PrimaryCorrection,
    over_range: OverRangeBand,
) -> (DiffMetrics, LinearParityMetrics, Vec<u8>) {
    let effect = correction_effect(1, correction);
    let expected = cpu_reference_monitor(frame, &[correction]);
    let expected_linear = cpu_reference_linear(frame, correction);
    assert_case_is_not_vacuous(
        &expected_linear,
        &cpu_reference_linear(frame, PrimaryCorrection::default()),
        correction,
    );
    let actual = compositor
        .render(
            (width, height),
            &[CompositorLayer {
                frame,
                effects: std::slice::from_ref(&effect),
                transition: TransitionRenderParams::default(),
            }],
        )
        .expect("production GPU compositor should render the CC1 fixture")
        .rgba;
    let monitor_metric = abs_code_diff_rgb(&actual, &expected);
    let actual_linear = compositor
        .render_working(
            (width, height),
            &[CompositorLayer {
                frame,
                effects: std::slice::from_ref(&effect),
                transition: TransitionRenderParams::default(),
            }],
        )
        .expect("production GPU working-surface readback");
    let linear_metric = linear_parity_metrics(&actual_linear, &expected_linear);
    assert!(
        linear_metric.compared() >= width as usize * height as usize,
        "GPU/CPU linear case has too few in-domain RGB samples: {linear_metric:?}"
    );
    assert!(
        monitor_metric.max <= MONITOR_CPU_GPU_MAX,
        "GPU/CPU monitor max metric for {:?}: {monitor_metric:?}",
        correction
    );
    assert!(
        monitor_metric.p99 <= MONITOR_CPU_GPU_P99,
        "GPU/CPU monitor P99 metric for {:?}: {monitor_metric:?}",
        correction
    );
    assert!(
        monitor_metric.mean <= MONITOR_CPU_GPU_MEAN,
        "GPU/CPU monitor mean metric for {:?}: {monitor_metric:?}",
        correction
    );
    match over_range {
        OverRangeBand::Required => assert!(
            linear_metric.over_range_samples > 0,
            "control case {correction:?} landed no sample in the §6.2 over-range band, so the over-range tolerances were never applied: {linear_metric:?}"
        ),
        OverRangeBand::ProvablyEmpty(reason) => assert_eq!(
            linear_metric.over_range_samples, 0,
            "control case {correction:?} was declared unable to reach the over-range band ({reason}) but produced samples there: {linear_metric:?}"
        ),
    }
    assert_linear_parity(&linear_metric, &format!("{correction:?}"));
    (monitor_metric, linear_metric, actual.as_ref().clone())
}

/// Which adapter actually produced a piece of CC1 GPU evidence.
///
/// CC1 §5/§6.1.7 make the renderer part of the claim, so the lane travels with
/// the evidence instead of being inferred from the test name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum GpuLane {
    /// `force_fallback_adapter` succeeded: lavapipe/llvmpipe/WARP.
    SoftwareFallback,
    /// No software adapter exists and the operator opted in to the physical
    /// adapter for the default lane via [`HARDWARE_GPU_OPT_IN_ENV`].
    HardwareOptIn,
    /// The explicit `--ignored` hardware lane.
    Hardware,
}

impl GpuLane {
    pub(crate) const fn id(self) -> &'static str {
        match self {
            Self::SoftwareFallback => "software_fallback",
            Self::HardwareOptIn => "hardware_optin",
            Self::Hardware => "hardware",
        }
    }
}

/// A GPU context plus the provenance that must be reported with, and asserted
/// against, every raster it renders.
pub(crate) struct FixtureGpu {
    context: GpuContext,
    info: String,
    pub(crate) lane: GpuLane,
    software_fallback: bool,
    gpu_claim: bool,
}

impl FixtureGpu {
    fn describe(context: GpuContext, lane: GpuLane) -> Self {
        let metadata = context.monitor_proof_metadata();
        let info = format!(
            "backend={};adapter={};software_fallback={};gpu_claim={};lane={}",
            metadata.backend,
            metadata.adapter,
            metadata.software_fallback,
            metadata.gpu_claim,
            lane.id(),
        );
        // Two adapters can be installed at once (lavapipe next to a physical
        // GPU). Printing the acquired adapter for every lane is what makes
        // "this lane ran where it claims to have run" checkable from a test
        // log instead of inferred from the test name.
        println!("CC_GPU_LANE lane={} {info}", lane.id());
        Self {
            context,
            info,
            lane,
            software_fallback: metadata.software_fallback,
            gpu_claim: metadata.gpu_claim,
        }
    }

    pub(crate) fn context(&self) -> GpuContext {
        self.context.clone()
    }

    pub(crate) fn backend(&self) -> &str {
        backend_name(&self.info)
    }

    /// Assert that a production proof rendered on this context reports the
    /// adapter that was actually acquired.  Hardcoding `software_fallback ==
    /// true` would make the assertion unsatisfiable on a machine without
    /// lavapipe and, worse, would let a hardware run keep claiming software
    /// provenance.
    pub(crate) fn assert_proof_provenance(&self, metadata: &kinewright_core::MonitorProofMetadata) {
        assert!(
            !metadata.backend.is_empty(),
            "monitor proof must name the backend that rendered it"
        );
        assert!(
            !metadata.adapter.is_empty(),
            "monitor proof must name the adapter that rendered it"
        );
        assert_eq!(
            metadata.software_fallback, self.software_fallback,
            "monitor proof software_fallback must match the acquired adapter ({})",
            self.info
        );
        assert_eq!(
            metadata.gpu_claim, self.gpu_claim,
            "monitor proof gpu_claim must match the acquired adapter ({})",
            self.info
        );
        assert_ne!(
            metadata.software_fallback, metadata.gpu_claim,
            "a proof cannot be both a software fallback and a GPU claim ({})",
            self.info
        );
    }
}

/// Acquire the adapter for the default (non-`--ignored`) CC1 GPU lane.
///
/// The anti-silent-skip design is intentional: a machine with no adapter must
/// fail, never report a green fixture it did not run.  A machine with a real
/// GPU but no software rasterizer can opt in to the physical adapter, and the
/// evidence then says so.
pub(crate) fn fallback_gpu() -> FixtureGpu {
    let software = match GpuContext::headless(true) {
        Ok(context) => context,
        Err(error) => return hardware_opt_in_gpu(&error.to_string()),
    };
    // The opt-in is a remedy for a machine with *no* software rasterizer, not
    // a lane switch. A machine that has both adapters must keep running this
    // lane on the software one, and must say so rather than ignore the
    // operator's environment silently.
    if std::env::var(HARDWARE_GPU_OPT_IN_ENV).ok().as_deref() == Some("1") {
        println!(
            "CC_GPU_LANE {HARDWARE_GPU_OPT_IN_ENV}=1 ignored: a software fallback adapter exists, so the default lane stays on it. Use the --ignored hardware lane for physical-adapter evidence."
        );
    }
    let fixture = FixtureGpu::describe(software, GpuLane::SoftwareFallback);
    #[cfg(target_os = "linux")]
    {
        let lower = fixture.info.to_ascii_lowercase();
        assert!(
            lower.contains("lavapipe") || lower.contains("llvmpipe"),
            "CC1 Linux software GPU evidence must use lavapipe/llvmpipe; adapter was {}. Install Mesa's software Vulkan adapter, fix Vulkan ICD discovery, or opt in to the physical adapter with {HARDWARE_GPU_OPT_IN_ENV}=1.",
            fixture.info
        );
    }
    assert!(
        fixture.software_fallback && !fixture.gpu_claim,
        "the software lane must report software provenance: {}",
        fixture.info
    );
    fixture
}

fn hardware_opt_in_gpu(software_error: &str) -> FixtureGpu {
    assert_eq!(
        std::env::var(HARDWARE_GPU_OPT_IN_ENV).ok().as_deref(),
        Some("1"),
        "CC1 primary GPU evidence requires a Linux lavapipe/WARP fallback adapter; no adapter was available ({software_error}). Install Mesa lavapipe and ensure Vulkan ICD discovery is enabled (for example, VK_ICD_FILENAMES), then rerun cargo test -p kinewright-media. On a machine that has a physical GPU but no software rasterizer, set {HARDWARE_GPU_OPT_IN_ENV}=1 to run this lane on the real adapter; the evidence then records software_fallback=false, gpu_claim=true, and lane=hardware_optin."
    );
    let context = GpuContext::headless(false).unwrap_or_else(|error| {
        panic!(
            "{HARDWARE_GPU_OPT_IN_ENV}=1 was set but no adapter was available at all (software: {software_error}; hardware: {error})."
        )
    });
    let fixture = FixtureGpu::describe(context, GpuLane::HardwareOptIn);
    assert!(
        !fixture.software_fallback && fixture.gpu_claim,
        "{HARDWARE_GPU_OPT_IN_ENV}=1 must acquire a real non-CPU adapter; observed {}. Check GPU drivers/ICD discovery.",
        fixture.info
    );
    fixture
}

pub(crate) fn hardware_gpu() -> FixtureGpu {
    let context = GpuContext::headless(false).unwrap_or_else(|error| {
        panic!(
            "CC1 hardware GPU parity is required but no non-fallback adapter was available ({error}). Install/enable a supported Vulkan, DX12, Metal, or GL adapter for this platform, then rerun cargo test -p kinewright-media."
        )
    });
    let fixture = FixtureGpu::describe(context, GpuLane::Hardware);
    assert!(
        !fixture.software_fallback && fixture.gpu_claim,
        "CC1 hardware GPU parity must use a real non-CPU adapter; observed {}. Check GPU drivers/ICD discovery.",
        fixture.info
    );
    fixture
}

fn backend_name(info: &str) -> &str {
    // Preserve the exact production provenance string, including backend,
    // adapter/device name, fallback status, and GPU claim.  Collapsing this to
    // a generic "wgpu_fallback" label makes a parity result unauditable.
    info
}

/// Compare one manifest colour-description object against the serialized
/// project-state description it claims to record.
///
/// The manifest is evidence, so a stale tolerance or a drifted target is a
/// fixture failure rather than a documentation nit.
fn assert_manifest_description(manifest: &Value, expected: &ColorDescription, label: &str) {
    let object = manifest
        .as_object()
        .unwrap_or_else(|| panic!("manifest {label} must be an object"));
    let serialized = serde_json::to_value(expected)
        .unwrap_or_else(|error| panic!("{label} description should serialize: {error}"));
    for field in [
        "primaries",
        "transfer",
        "matrix",
        "range",
        "white_point",
        "bit_depth",
    ] {
        assert_eq!(
            object.get(field),
            serialized.get(field),
            "manifest {label}.{field} does not match ColorContext::sdr_rec709()"
        );
    }
}

/// Assert one declared manifest tolerance equals the `f64` code constant that
/// the fixtures actually gate with.
pub(crate) fn assert_manifest_f64(parent: &Value, key: &str, expected: f64) {
    let declared = parent
        .get(key)
        .and_then(Value::as_f64)
        .unwrap_or_else(|| panic!("manifest must declare a numeric {key}"));
    assert_eq!(
        declared, expected,
        "manifest {key} does not match the code constant"
    );
}

/// The `f32` form. The declared decimal must round to exactly the constant so
/// the manifest cannot drift by a value smaller than a single-precision step.
pub(crate) fn assert_manifest_f32(parent: &Value, key: &str, expected: f32) {
    let declared = parent
        .get(key)
        .and_then(Value::as_f64)
        .unwrap_or_else(|| panic!("manifest must declare a numeric {key}"));
    assert_eq!(
        declared as f32, expected,
        "manifest {key} does not match the code constant"
    );
}

#[test]
fn cc1_manifest_declares_every_required_evidence_fixture() {
    let manifest: Value = serde_json::from_str(include_str!("../tests/fixtures/cc1_manifest.json"))
        .expect("CC1 fixture manifest must be valid JSON");
    assert_eq!(manifest["manifest_version"], 1);
    // The manifest describes the same profile and control tables the code
    // validates against, not just the right number of entries.
    assert_eq!(
        manifest["profiles"],
        json!([
            ColorSourceProfile::Rec709Video.id(),
            ColorSourceProfile::SrgbFull.id()
        ]),
    );
    assert_eq!(
        manifest["controls"],
        Value::Array(
            PrimaryParameter::ALL
                .into_iter()
                .map(|parameter| Value::String(parameter.name().to_owned()))
                .collect()
        ),
    );
    assert_eq!(manifest["source_depths_bits"], json!([8, 10]));

    // Working, monitoring, and delivery are project state (§2). The manifest
    // must record the same descriptions the renderer selects from.
    let context = ColorContext::sdr_rec709();
    assert_manifest_description(&manifest["working"], &context.working, "working");
    assert_manifest_description(&manifest["monitoring"], &context.monitoring, "monitoring");
    assert_manifest_description(&manifest["delivery"], &context.delivery, "delivery");
    assert_eq!(manifest["delivery"]["codec"], "h264");
    assert_eq!(manifest["delivery"]["pixel_format"], "yuv420p");
    // The 16-bit intermediate that carries the compositor's single
    // quantization into the export filter graph. `libswscale` reads 16-bit RGB
    // on the `255 << 8` scale, so the manifest records the exact code the
    // encoder emits for nominal white; 65_535 would encode to luma 236.
    assert_eq!(
        manifest["delivery"]["intermediate_white"],
        json!(DELIVERY_INTERMEDIATE_WHITE),
        "manifest delivery.intermediate_white does not match DELIVERY_INTERMEDIATE_WHITE"
    );

    // Every declared tolerance must be the constant the fixtures actually
    // assert with, so the manifest cannot advertise a gate nothing enforces.
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
    assert_manifest_f64(
        tolerances,
        "delivery_codec_max_code",
        f64::from(DELIVERY_CODEC_MAX),
    );
    assert_manifest_f64(tolerances, "delivery_codec_p99_code", DELIVERY_CODEC_P99);
    assert_manifest_f64(tolerances, "delivery_codec_mean_code", DELIVERY_CODEC_MEAN);
    // The two-node recovery bound is its own gate, not the CPU/GPU gate.
    assert_manifest_f32(tolerances, "no_clamp_recovery_max", NO_CLAMP_RECOVERY_MAX);
    assert_eq!(
        tolerances.as_object().map(serde_json::Map::len),
        Some(12),
        "an undeclared tolerance key would not be asserted anywhere"
    );

    let ramp_gate = &manifest["identity_ramp_gate"];
    assert_manifest_f64(ramp_gate, "max_code", f64::from(IDENTITY_RAMP_MONITOR_MAX));
    assert_manifest_f64(ramp_gate, "p99_code", IDENTITY_RAMP_MONITOR_P99);
    assert_manifest_f64(ramp_gate, "mean_code", IDENTITY_RAMP_MONITOR_MEAN);
    assert_manifest_f32(ramp_gate, "swscale_linear_max", IDENTITY_RAMP_SWSCALE_MAX);
    assert_manifest_f32(ramp_gate, "swscale_linear_p99", IDENTITY_RAMP_SWSCALE_P99);
    assert_manifest_f32(ramp_gate, "swscale_linear_mean", IDENTITY_RAMP_SWSCALE_MEAN);
    assert_eq!(ramp_gate["alpha"], "exact");
    assert_eq!(ramp_gate["native_plane_round_trip"], "exact");

    // The GPU lane descriptions must name the lanes the fixtures can actually
    // take, including the hardware opt-in for machines with no rasterizer.
    let gpu_contexts = &manifest["gpu_contexts"];
    let software = gpu_contexts["software"]
        .as_str()
        .expect("software GPU lane description");
    assert!(software.contains("lavapipe"), "software lane: {software}");
    let opt_in = gpu_contexts["software_unavailable_opt_in"]
        .as_str()
        .expect("hardware opt-in lane description");
    assert!(
        opt_in.contains(HARDWARE_GPU_OPT_IN_ENV) && opt_in.contains(GpuLane::HardwareOptIn.id()),
        "opt-in lane: {opt_in}"
    );
    let hardware = gpu_contexts["hardware"]
        .as_str()
        .expect("hardware GPU lane description");
    assert!(
        hardware.contains("cc1_gpu_compositor_matches_canonical_cpu_reference_on_hardware"),
        "hardware lane: {hardware}"
    );

    let required = manifest["required_evidence"]
        .as_array()
        .expect("required evidence list");
    for name in [
        "migration",
        "identity_ramps",
        "neutral_chart",
        "primary_controls",
        "gpu_cpu_parity",
        "no_intermediate_clamp",
        "unsupported_metadata",
        "same_raster_monitor_proof",
        "h264_yuv420p_delivery",
        "managed_cache_memory_bound",
    ] {
        assert!(required.iter().any(|value| value == name), "missing {name}");
    }
    emit_evidence(
        "cc1_manifest",
        "none",
        None,
        None,
        json!({"manifest_version": 1}),
        (0, 0),
        None,
        output_hash(manifest.to_string().as_bytes()),
        json!({
            "required_fixture_count": required.len(),
            "tolerances_matched_code_constants": true,
            "targets_matched_project_state": true,
        }),
    );
}

#[test]
fn cc1_core_migration_fixture_preserves_effect_order_and_parameters() {
    let legacy = Effect {
        id: EffectId(7),
        name: "color_grade".to_owned(),
        parameters: BTreeMap::from([
            ("exposure_milli_stops".to_owned(), ParamValue::Integer(750)),
            (
                "label".to_owned(),
                ParamValue::Text("preserve me".to_owned()),
            ),
        ]),
        keyframes: BTreeMap::new(),
    };
    let mut document = Document::default();
    document.tracks.push(Track {
        id: TrackId(1),
        kind: TrackKind::Video,
        sync_lock: true,
        clips: vec![Clip {
            id: ClipId(1),
            asset: AssetId(1),
            source_range: TimeCode::ZERO..TimeCode(10),
            content: ClipContent::Media,
            timeline_start: TimeCode::ZERO,
            effects: vec![
                Effect {
                    id: EffectId(6),
                    name: "brightness".to_owned(),
                    parameters: BTreeMap::new(),
                    keyframes: BTreeMap::new(),
                },
                legacy.clone(),
                Effect {
                    id: EffectId(8),
                    name: "saturation".to_owned(),
                    parameters: BTreeMap::new(),
                    keyframes: BTreeMap::new(),
                },
            ],
            transition_in: None,
            link: None,
            audio_gain_tenth_db: 0,
            audio_fade_in_frames: TimeCode::ZERO,
            audio_fade_out_frames: TimeCode::ZERO,
            speed_percent: 100,
        }],
    });
    let wire = serde_json::to_value(&document).expect("migration document should serialize");
    assert_eq!(
        wire["tracks"][0]["clips"][0]["effects"][1]["name"],
        "color_grade"
    );
    let decoded: Document = serde_json::from_value(wire).expect("legacy document should decode");
    let effects = &decoded.tracks[0].clips[0].effects;
    assert_eq!(effects[1].name, "primary_correction");
    assert_eq!(effects[1].parameters, legacy.parameters);
    assert_eq!(
        effects
            .iter()
            .map(|effect| effect.name.as_str())
            .collect::<Vec<_>>(),
        vec!["brightness", "primary_correction", "saturation"]
    );
    // Exercise both the pre-CC0 omission and the exact CC0 placeholder at
    // the Core serde boundary.
    let mut pre_cc0 = serde_json::to_value(Document::default()).expect("pre-CC0 document");
    pre_cc0
        .as_object_mut()
        .expect("document object")
        .remove("color_context");
    let pre_cc0_reopened: Document =
        serde_json::from_value(pre_cc0).expect("pre-CC0 document should reopen");
    assert_eq!(pre_cc0_reopened.color_context, ColorContext::sdr_rec709());
    let cc0_context = serde_json::json!({
        "working": cc0_application_description(ColorMatrix::Rgb, ColorRange::Full),
        "monitoring": cc0_application_description(ColorMatrix::Rgb, ColorRange::Full),
        "delivery": cc0_application_description(ColorMatrix::Bt709, ColorRange::Limited),
    });
    let migrated_cc0: ColorContext =
        serde_json::from_value(cc0_context).expect("CC0 context should migrate");
    assert_eq!(migrated_cc0, ColorContext::sdr_rec709());
    assert!(migrated_cc0.is_managed_sdr_compatible());
    assert!(is_legacy_display_effect("brightness"));
    assert_eq!(
        effect_compatibility_stage("brightness"),
        Some(kinewright_core::EffectCompatibilityStage::LegacyDisplayCoded)
    );
    assert_eq!(
        effect_compatibility_stage("look_lut"),
        Some(kinewright_core::EffectCompatibilityStage::PostPrimaryLut)
    );
    let saved = serde_json::to_vec(&decoded).expect("save migrated document");
    let reopened: Document = serde_json::from_slice(&saved).expect("reopen migrated document");
    assert_eq!(reopened, decoded);
    // Journal replay and history use the same public Core actor boundary as
    // the application, including undo and redo events.
    let migration_asset = MediaAsset {
        id: AssetId(22),
        path: PathBuf::from("cc1-migration-fixture.mp4"),
        name: "migration fixture".to_owned(),
        duration: TimeCode(30),
        fps: Rational::new(30, 1).expect("migration fps"),
        kind: MediaKind::Video,
        resolution: Some((16, 16)),
        source_fingerprint: kinewright_core::MediaSourceFingerprint::default(),
        color_description: ColorDescription::default(),
    };
    let add_asset = Operation::AddAsset {
        asset: migration_asset.clone(),
    };
    let core = Core::spawn(Document::default()).expect("migration core");
    let add_event = core
        .request(Command::Do(add_asset.clone()))
        .expect("journal add asset");
    assert!(matches!(
        add_event,
        Event::DocumentChanged {
            journal_command: Some(JournalCommand::Do(_)),
            ..
        }
    ));
    let mut replay_legacy_effect = legacy.clone();
    replay_legacy_effect.parameters.remove("label");
    let legacy_journal = JournalCommand::Do(Operation::AddEffect {
        clip: ClipId(1),
        effect: replay_legacy_effect.clone(),
    });
    let replay_wire =
        serde_json::to_value(legacy_journal).expect("legacy journal should serialize");
    let replay: JournalCommand =
        serde_json::from_value(replay_wire).expect("legacy journal should deserialize");
    let JournalCommand::Do(Operation::AddEffect {
        clip: replay_clip,
        effect: replay_effect,
    }) = &replay
    else {
        panic!("legacy journal should retain an AddEffect operation");
    };
    assert_eq!(*replay_clip, ClipId(1));
    assert_eq!(replay_effect.name, "primary_correction");
    assert_eq!(replay_effect.parameters, replay_legacy_effect.parameters);
    let mut replay_seed = document.clone();
    replay_seed.tracks[0].clips[0].effects.truncate(1);
    let mut replay_asset = migration_asset.clone();
    replay_asset.id = AssetId(1);
    replay_seed.media_pool.push(replay_asset);
    replay_seed.fps = Rational::new(30, 1).expect("replay fps");
    replay_seed.duration = TimeCode(10);
    let replay_core = Core::spawn(replay_seed).expect("replay core");
    let replay_event = replay_core
        .request(replay.clone().into())
        .expect("legacy journal replay should apply");
    let replayed_document = match replay_event {
        Event::DocumentChanged { doc, .. } => doc,
        other => panic!("legacy journal replay was not accepted: {other:?}"),
    };
    let replayed_effects = &replayed_document.tracks[0].clips[0].effects;
    assert_eq!(
        replayed_effects
            .iter()
            .map(|effect| effect.name.as_str())
            .collect::<Vec<_>>(),
        vec!["brightness", "primary_correction"]
    );
    assert_eq!(
        replayed_effects[1].parameters,
        replay_legacy_effect.parameters
    );
    let override_operation = Operation::SetAssetColorDescription {
        asset: migration_asset.id,
        color_description: rec709_description(8, ColorRange::Limited, ColorTransfer::Bt709),
    };
    let changed = core
        .request(Command::Do(override_operation))
        .expect("migration override");
    assert!(matches!(changed, Event::DocumentChanged { .. }));
    let undone = core.request(Command::Undo).expect("migration undo");
    let undone_document = match undone {
        Event::DocumentChanged {
            doc,
            journal_command: Some(JournalCommand::Undo),
            ..
        } => doc,
        other => panic!("migration undo was not an accepted document state: {other:?}"),
    };
    assert!(
        undone_document
            .asset(migration_asset.id)
            .expect("undo asset")
            .color_description
            .is_unknown()
    );
    let redone = core.request(Command::Redo).expect("migration redo");
    let redone_document = match redone {
        Event::DocumentChanged {
            doc,
            journal_command: Some(JournalCommand::Redo),
            ..
        } => doc,
        other => panic!("migration redo was not an accepted document state: {other:?}"),
    };
    assert_eq!(
        redone_document
            .asset(migration_asset.id)
            .expect("redo asset")
            .color_description,
        rec709_description(8, ColorRange::Limited, ColorTransfer::Bt709)
    );
    emit_evidence(
        "cc1_migration",
        "kinewright_core_serde",
        None,
        None,
        json!({"legacy_wire_name": "color_grade", "canonical_name": "primary_correction"}),
        (0, 0),
        None,
        output_hash(
            serde_json::to_string(&decoded)
                .expect("migration hash")
                .as_bytes(),
        ),
        json!({"vector_position_preserved": true, "parameters_preserved": true, "pre_cc0_reopen": true, "cc0_context_migrated": true, "legacy_display_effects_classified": true, "save_reopen": true, "journal_replay": true, "undo": true, "redo": true, "core_coverage": ["legacy_effect_migration_preserves_project_vector_position", "exact_old_cc0_context_migrates_to_managed_sdr_v1", "legacy_effect_inside_journal_operation_migrates_on_decode", "color_override_events_journal_and_history_preserve_the_exact_operation"]}),
    );
}

#[test]
fn cc1_identity_ramps_decode_actual_sources_to_working_and_monitor() {
    initialize_ffmpeg().expect("FFmpeg must initialize for CC1 source fixtures");
    let directory = TempDirectory::new("cc1-identity-ramps");
    let mut evidence_hash = Vec::new();
    for spec in ramp_specs() {
        let description = source_description_for(&spec);
        assert_eq!(classify_source(&description), Ok(spec.profile));
        let (path, width, source_bytes) = generate_ramp_media(&directory, &spec);
        let (asset, frame) = decode_actual_ramp(&path, width, &description);
        assert_eq!(asset.resolution, Some((width, 1)));
        let actual = monitor_ramp(&frame);
        let expected = expected_ramp_monitor(&spec);
        assert_monotonic_ramp(&actual, width);
        assert_neutral_pixels(&actual, spec.name);
        let metric = abs_code_diff_rgb(&actual, &expected);
        assert!(
            metric.max <= IDENTITY_RAMP_MONITOR_MAX,
            "{} source/reference ramp differs by {:?}; source hash={}",
            spec.name,
            metric,
            output_hash(&source_bytes)
        );
        assert!(
            metric.p99 <= IDENTITY_RAMP_MONITOR_P99,
            "{} ramp P99 metric: {metric:?}",
            spec.name
        );
        assert!(
            metric.mean <= IDENTITY_RAMP_MONITOR_MEAN,
            "{} ramp mean metric: {metric:?}",
            spec.name
        );
        let first = &actual[..4];
        let last = &actual[actual.len() - 4..];
        assert!(first[0] <= 1 && first[1] <= 1 && first[2] <= 1);
        assert!(last[0] >= 254 && last[1] >= 254 && last[2] >= 254);
        if matches!(&spec.range, ColorRange::Limited) {
            assert!(actual[..3].iter().all(|value| *value <= 1));
            assert!(
                actual[actual.len() - 4..actual.len() - 1]
                    .iter()
                    .all(|value| *value >= 254)
            );
        }
        let working_values = frame
            .pixels
            .as_chunks::<4>()
            .0
            .iter()
            .flat_map(|rgba| [rgba[0].to_f32(), rgba[1].to_f32(), rgba[2].to_f32()])
            .collect::<Vec<_>>();
        let expected_working = (0..width)
            .flat_map(|code| {
                let encoded =
                    ramp_native_code(&spec, code) as f32 / ((1_u32 << spec.depth) - 1) as f32;
                let coded = if matches!(&spec.range, ColorRange::Limited) {
                    expand_native_range(
                        [encoded; 3],
                        &ColorBitDepth::Integer(u16::from(spec.depth)),
                        &ColorRange::Limited,
                    )
                    .expect("limited source expansion")
                } else {
                    [encoded; 3]
                };
                let decoded = match spec.encoding {
                    RampEncoding::Rec709 => coded.map(decode_bt709),
                    RampEncoding::Srgb => coded.map(decode_srgb),
                };
                decoded.into_iter()
            })
            .collect::<Vec<_>>();
        let float_metric = abs_float_diff(&working_values, &expected_working);
        // The swscale RGBA64 boundary is compared against the §3.1 native-code
        // reference equations, so all three named moments are asserted rather
        // than one loose maximum borrowed from the CPU/GPU gate.
        assert!(
            float_metric.max <= IDENTITY_RAMP_SWSCALE_MAX,
            "{} working ramp max differs from the source reference by {:?}",
            spec.name,
            float_metric
        );
        assert!(
            float_metric.p99 <= IDENTITY_RAMP_SWSCALE_P99,
            "{} working ramp P99 differs from the source reference by {:?}",
            spec.name,
            float_metric
        );
        assert!(
            float_metric.mean <= IDENTITY_RAMP_SWSCALE_MEAN,
            "{} working ramp mean differs from the source reference by {:?}",
            spec.name,
            float_metric
        );
        evidence_hash.extend_from_slice(&actual);
        emit_evidence(
            spec.name,
            "ffmpeg_swscale_rgba64le",
            Some(spec.profile),
            Some(spec.depth),
            json!({"primary_correction": "neutral"}),
            (width, 1),
            Some(file_hash(&path)),
            output_hash(&actual),
            json!({
                "monitor_max_code_error": metric.max,
                "monitor_p99_code_error": metric.p99,
                "monitor_mean_code_error": metric.mean,
                "working_max_error": float_metric.max,
                "working_p99_error": float_metric.p99,
                "working_mean_error": float_metric.mean,
                "source_description_raw": asset.color_description,
                "first_luma": actual[0],
                "last_luma": actual[actual.len() - 4],
                "monotonic": true,
                "neutral": true,
            }),
        );
    }
    assert!(!evidence_hash.is_empty());
    emit_evidence(
        "cc1_identity_ramps_summary",
        "ffmpeg_swscale_rgba64le",
        None,
        None,
        json!({"primary_correction": "neutral"}),
        (1024, 1),
        None,
        output_hash(&evidence_hash),
        json!({"ramp_count": ramp_specs().len()}),
    );
}

/// The §3.1 monitor codes for the twelve reference patches under a neutral
/// correction, computed by hand from the specification equations.
///
/// Keeping the literal table next to the analytic computation means a change
/// to either the spec transcription or the production encoder is a failure,
/// not a silently updated expectation.
const NEUTRAL_CHART_MONITOR_CODES: [[u8; 3]; 12] = [
    [0, 0, 0],       // black:      4.5 * 0
    [11, 11, 11],    // near_black: round(255 * 4.5 * 0.01)
    [104, 104, 104], // gray_18:    round(255 * (1.099 * 0.18^0.45 - 0.099))
    [180, 180, 180], // mid_gray:   round(255 * (1.099 * 0.50^0.45 - 0.099))
    [242, 242, 242], // near_white: round(255 * (1.099 * 0.90^0.45 - 0.099))
    [255, 255, 255], // white:      1.099 - 0.099 = 1.0
    [255, 0, 0],
    [0, 255, 0],
    [0, 0, 255],
    [0, 255, 255],
    [255, 0, 255],
    [255, 255, 0],
];

/// The Rec.709 luma monitor code each patch collapses to at saturation -100.
const CHART_MONOCHROME_MONITOR_CODES: [u8; 12] =
    [0, 11, 104, 180, 242, 255, 114, 216, 61, 226, 134, 246];

fn chart_correction(parameter: PrimaryParameter, value: i64) -> PrimaryCorrection {
    PrimaryCorrection::from_effect(&effect_with_parameters(700, [(parameter.name(), value)]))
        .expect("chart control must be inside the descriptor bounds")
}

fn chart_patch_codes(output: &[u8], index: usize) -> [u8; 3] {
    [
        output[index * 3],
        output[index * 3 + 1],
        output[index * 3 + 2],
    ]
}

#[test]
fn cc1_neutral_chart_matches_analytic_spec_codes_for_twelve_reference_patches() {
    let patches = chart_patches();
    // 1. The analytic table and the hand-computed literals must agree.
    for (index, (name, input)) in patches.into_iter().enumerate() {
        let analytic = input.map(|value| spec_monitor_code_f64(f64::from(value)));
        assert_eq!(
            analytic, NEUTRAL_CHART_MONITOR_CODES[index],
            "{name} analytic §3.1 monitor code drifted from the recorded expectation"
        );
    }
    // 2. The production encoder must produce exactly those codes.
    let output = chart_monitor(PrimaryCorrection::default());
    assert_eq!(output.len(), 12 * 3);
    for (index, (name, _)) in patches.into_iter().enumerate() {
        assert_eq!(
            chart_patch_codes(&output, index),
            NEUTRAL_CHART_MONITOR_CODES[index],
            "{name} monitor code does not match the §3.1 expectation"
        );
    }
    // 3. §6.2 channel neutrality and the range endpoints.
    let neutral_spread = chart_neutral_spread(&output);
    assert!(
        neutral_spread <= 1,
        "neutral chart spread: {neutral_spread}"
    );
    assert_eq!(chart_patch_codes(&output, 0), [0, 0, 0]);
    assert_eq!(chart_patch_codes(&output, 5), [255, 255, 255]);
    emit_evidence(
        "cc1_neutral_chart_12_patch",
        "cpu_reference",
        Some(ColorSourceProfile::Rec709Video),
        Some(8),
        json!({"primary_correction": "neutral", "patches": 12}),
        (12, 1),
        None,
        output_hash(&output),
        json!({
            "neutral_channel_spread_max": neutral_spread,
            "patch_count": 12,
            "expected_monitor_codes": NEUTRAL_CHART_MONITOR_CODES,
            "expectation_source": "cc1 3.1 equations evaluated in f64 inside the fixture",
        }),
    );
}

#[test]
fn cc1_neutral_chart_white_balance_and_saturation_move_in_the_documented_direction() {
    let patches = chart_patches();
    let neutral = chart_monitor(PrimaryCorrection::default());

    // Temperature: positive is warmer, so red gains 10% and blue loses 10%.
    // Green is a temperature identity in linear light, before any encoding.
    let mut temperature_moves = 0_u32;
    for signed in [100_i64, -100] {
        let correction = chart_correction(PrimaryParameter::TemperaturePercent, signed);
        let output = chart_monitor(correction);
        for (index, (name, input)) in patches.into_iter().enumerate() {
            let linear = correction
                .apply_checked(input)
                .expect("temperature control");
            assert_matches_spec_f64(
                linear[1],
                spec_apply_primary_f64(correction, input.map(f64::from))[1],
                &format!("{name} green under temperature {signed}"),
            );
            assert_matches_spec_f64(
                linear[1],
                f64::from(input[1]),
                &format!("{name} green must be a temperature identity in linear light"),
            );
            let actual = chart_patch_codes(&output, index);
            let expected = NEUTRAL_CHART_MONITOR_CODES[index];
            if signed > 0 {
                assert!(
                    actual[0] >= expected[0],
                    "{name} red fell on a warmer chart"
                );
                assert!(
                    actual[2] <= expected[2],
                    "{name} blue rose on a warmer chart"
                );
            } else {
                assert!(
                    actual[0] <= expected[0],
                    "{name} red rose on a cooler chart"
                );
                assert!(
                    actual[2] >= expected[2],
                    "{name} blue fell on a cooler chart"
                );
            }
            assert_eq!(
                actual[1], expected[1],
                "{name} green code changed under a temperature-only control"
            );
            if actual[0] != expected[0] || actual[2] != expected[2] {
                temperature_moves += 1;
            }
        }
    }
    assert!(
        temperature_moves >= 12,
        "temperature +/-100 barely moved the chart: {temperature_moves} patch moves"
    );

    // Tint: positive is magenta, which is green *down*. Red and blue are tint
    // identities in linear light.
    let mut tint_moves = 0_u32;
    for signed in [100_i64, -100] {
        let correction = chart_correction(PrimaryParameter::TintPercent, signed);
        let output = chart_monitor(correction);
        for (index, (name, input)) in patches.into_iter().enumerate() {
            let linear = correction.apply_checked(input).expect("tint control");
            for channel in [0_usize, 2] {
                assert_matches_spec_f64(
                    linear[channel],
                    f64::from(input[channel]),
                    &format!("{name} channel {channel} must be a tint identity in linear light"),
                );
            }
            let actual = chart_patch_codes(&output, index);
            let expected = NEUTRAL_CHART_MONITOR_CODES[index];
            if signed > 0 {
                assert!(
                    actual[1] <= expected[1],
                    "{name} green rose on a magenta tint"
                );
            } else {
                assert!(
                    actual[1] >= expected[1],
                    "{name} green fell on a green tint"
                );
            }
            assert_eq!(
                [actual[0], actual[2]],
                [expected[0], expected[2]],
                "{name} red/blue codes changed under a tint-only control"
            );
            if actual[1] != expected[1] {
                tint_moves += 1;
            }
        }
    }
    assert!(
        tint_moves >= 8,
        "tint +/-100 barely moved the chart: {tint_moves} patch moves"
    );

    // Saturation -100 collapses every patch to its Rec.709 luma code.
    let monochrome = chart_correction(PrimaryParameter::SaturationPercent, -100);
    let monochrome_output = chart_monitor(monochrome);
    for (index, (name, input)) in patches.into_iter().enumerate() {
        let luma = spec_luma_f64(input.map(f64::from));
        let expected = spec_monitor_code_f64(luma);
        assert_eq!(
            expected, CHART_MONOCHROME_MONITOR_CODES[index],
            "{name} analytic luma code drifted from the recorded expectation"
        );
        let actual = chart_patch_codes(&monochrome_output, index);
        for channel in actual {
            assert!(
                channel.abs_diff(expected) <= 1,
                "{name} did not collapse to its Rec.709 luma code: {actual:?} vs {expected}"
            );
        }
        assert!(
            actual[0].abs_diff(actual[1]) <= 1 && actual[1].abs_diff(actual[2]) <= 1,
            "{name} is not monochrome at saturation -100: {actual:?}"
        );
    }

    // Saturation +100 must push every chroma patch away from its luma and
    // leave neutral patches alone.
    let saturated = chart_correction(PrimaryParameter::SaturationPercent, 100);
    let saturated_output = chart_monitor(saturated);
    let mut widened = 0_u32;
    for (index, (name, input)) in patches.into_iter().enumerate() {
        let luma = spec_luma_f64(input.map(f64::from));
        let expected =
            input.map(|channel| spec_monitor_code_f64(luma + (f64::from(channel) - luma) * 2.0));
        let actual = chart_patch_codes(&saturated_output, index);
        for channel in 0..3 {
            assert!(
                actual[channel].abs_diff(expected[channel]) <= 1,
                "{name} channel {channel} at saturation +100: {actual:?} vs {expected:?}"
            );
        }
        let neutral_patch = input[0] == input[1] && input[1] == input[2];
        if neutral_patch {
            assert_eq!(
                actual, NEUTRAL_CHART_MONITOR_CODES[index],
                "{name} is neutral and must be a saturation identity"
            );
        } else {
            widened += 1;
        }
    }
    assert_eq!(widened, 6, "the chart must contain six chroma patches");

    emit_evidence(
        "cc1_neutral_chart_white_balance_and_saturation",
        "cpu_reference",
        Some(ColorSourceProfile::Rec709Video),
        Some(8),
        json!({
            "temperature_percent": [100, -100],
            "tint_percent": [100, -100],
            "saturation_percent": [-100, 100],
        }),
        (12, 1),
        None,
        output_hash(&[neutral.as_slice(), &monochrome_output, &saturated_output].concat()),
        json!({
            "temperature_patch_moves": temperature_moves,
            "tint_patch_moves": tint_moves,
            "monochrome_luma_codes": CHART_MONOCHROME_MONITOR_CODES,
            "chroma_patches": widened,
        }),
    );
}

fn assert_chart_gpu_case(
    compositor: &Compositor,
    correction: PrimaryCorrection,
    over_range: OverRangeBand,
) -> Value {
    let (width, height, frame) = chart_frame();
    let (monitor, linear, _) =
        assert_gpu_control_case(compositor, width, height, &frame, correction, over_range);
    json!({
        "controls": correction_value_json(correction),
        "monitor_max_code_error": monitor.max,
        "monitor_p99_code_error": monitor.p99,
        "monitor_mean_code_error": monitor.mean,
        "linear": linear.as_json(),
        "over_range_band": over_range.as_json(),
    })
}

#[test]
fn cc1_neutral_chart_matches_the_production_compositor_under_the_cpu_gpu_gate() {
    // Unlike the representative bar fixture, every patch of the 12-patch chart
    // is in gamut, so the over-range band exists here only where a control
    // lifts a patch above 1.0. Each case states which it is.
    const CHART_IS_IN_GAMUT: &str = "every patch of the 12-patch chart is in gamut and this control does not lift one above 1.0";
    let gpu = fallback_gpu();
    let compositor = Compositor::new(gpu.context());
    let (width, height, _) = chart_frame();
    let mut cases = vec![json!({
        "case": "neutral",
        "metrics": assert_chart_gpu_case(
            &compositor,
            PrimaryCorrection::default(),
            OverRangeBand::ProvablyEmpty(CHART_IS_IN_GAMUT),
        ),
    })];
    for (parameter, value, over_range) in [
        (
            PrimaryParameter::TemperaturePercent,
            100_i64,
            OverRangeBand::Required,
        ),
        (
            PrimaryParameter::TemperaturePercent,
            -100,
            OverRangeBand::Required,
        ),
        (
            PrimaryParameter::TintPercent,
            100,
            OverRangeBand::ProvablyEmpty(CHART_IS_IN_GAMUT),
        ),
        (PrimaryParameter::TintPercent, -100, OverRangeBand::Required),
        (
            PrimaryParameter::SaturationPercent,
            -100,
            OverRangeBand::ProvablyEmpty(
                "full desaturation replaces every patch with its own luma, which is in gamut",
            ),
        ),
        (
            PrimaryParameter::SaturationPercent,
            100,
            OverRangeBand::Required,
        ),
    ] {
        cases.push(json!({
            "case": "single_control",
            "parameter": parameter.name(),
            "value": value,
            "metrics": assert_chart_gpu_case(
                &compositor,
                chart_correction(parameter, value),
                over_range,
            ),
        }));
    }
    emit_evidence(
        "cc1_neutral_chart_gpu_parity",
        gpu.backend(),
        Some(ColorSourceProfile::Rec709Video),
        Some(16),
        json!({"patches": 12, "patch_block_width": 8}),
        (width, height),
        None,
        output_hash(
            serde_json::to_string(&cases)
                .expect("chart evidence")
                .as_bytes(),
        ),
        json!({
            "lane": gpu.lane.id(),
            "cases": cases,
            "monitor_gate": {"max": MONITOR_CPU_GPU_MAX, "p99": MONITOR_CPU_GPU_P99, "mean": MONITOR_CPU_GPU_MEAN},
            "linear_gate": {
                "in_gamut": {"max": LINEAR_CPU_GPU_MAX, "p99": LINEAR_CPU_GPU_P99, "mean": LINEAR_CPU_GPU_MEAN},
                "over_range": {"max": LINEAR_CPU_GPU_MAX, "p99": LINEAR_OVER_RANGE_P99, "mean": LINEAR_OVER_RANGE_MEAN},
            },
        }),
    );
}

fn control_fixture_values(parameter: PrimaryParameter) -> Vec<i64> {
    let (min, max, neutral) = parameter.bounds();
    if parameter == PrimaryParameter::ExposureMilliStops {
        return vec![neutral, min, max, -1_000, 1_000];
    }
    let interior = match parameter {
        PrimaryParameter::ContrastPivotBasisPoints => 4_200,
        PrimaryParameter::SaturationPercent => -40,
        _ => 35,
    };
    vec![neutral, min, max, interior]
}

#[test]
fn cc1_primary_controls_match_the_spec_equations_at_neutral_bounds_and_interiors() {
    // §6.1.3 low/high tonal patch and a general chroma patch. Both are
    // evaluated against the §3.2 equations written out in f64 above.
    const TONAL_INPUT: [f32; 3] = [0.04, 0.5, 0.96];
    const CHROMA_INPUT: [f32; 3] = [0.23, 0.47, 0.81];

    let mut evidence = BTreeMap::new();
    for (index, parameter) in PrimaryParameter::ALL.into_iter().enumerate() {
        let values = control_fixture_values(parameter);
        let mut outputs = Vec::new();
        let mut expectations = Vec::new();
        for &value in &values {
            // Use the same case construction as the GPU parity sweep: sweeping
            // `contrast_pivot_basis_points` at `contrast_percent = 0` measures
            // an identity and proves nothing about the pivot.
            let case_parameters = control_case_parameters(parameter, value);
            let effect = effect_with_parameters(100 + index as u64, case_parameters.clone());
            let correction = PrimaryCorrection::from_effect(&effect).expect("control fixture");
            // Every control is evaluated on both patches so a tonal weight and
            // a chroma response are covered for each one.
            let mut case_output = Vec::new();
            let mut case_neutral = Vec::new();
            for input in [TONAL_INPUT, CHROMA_INPUT] {
                let output = correction.apply_checked(input).expect("valid control");
                let expected = spec_apply_primary_f64(correction, input.map(f64::from));
                for channel in 0..3 {
                    assert_matches_spec_f64(
                        output[channel],
                        expected[channel],
                        &format!("{case_parameters:?} on {input:?} channel {channel}"),
                    );
                }
                // Alpha is carried only so the shared vacuous-case guard can
                // read these as RGBA chunks; it is never compared.
                case_output.extend(output);
                case_output.push(1.0);
                case_neutral.extend(input);
                case_neutral.push(1.0);
                outputs.extend(output);
                expectations.extend(expected);
            }
            // The CPU lane needs the same guard as the GPU lane: a case whose
            // output is indistinguishable from its input reports a flattering
            // zero spec error while proving nothing about the control.
            assert_case_is_not_vacuous(&case_output, &case_neutral, correction);
        }
        // Neutral values are exact identities, including a neutral 0.5 pivot.
        let neutral_effect =
            effect_with_parameters(200 + index as u64, [(parameter.name(), values[0])]);
        let neutral = PrimaryCorrection::from_effect(&neutral_effect).expect("neutral control");
        assert_eq!(
            neutral,
            PrimaryCorrection::default(),
            "{} neutral value must resolve to the neutral correction",
            parameter.name()
        );
        let neutral_output = neutral.apply_checked(CHROMA_INPUT).expect("neutral input");
        for (actual, expected) in neutral_output.into_iter().zip(CHROMA_INPUT) {
            assert!((actual - expected).abs() <= 1.0e-6);
        }
        evidence.insert(
            parameter.name(),
            json!({"values": values, "outputs": outputs, "spec_f64_expectations": expectations}),
        );
    }
    assert_eq!(evidence.len(), 10);

    // Replace self-referential evidence flags with assertions on outputs.
    //
    // Exposure: +1000 milli-stops is exactly one stop, so linear light doubles.
    let plus_one_stop = PrimaryCorrection {
        exposure_milli_stops: 1_000,
        ..PrimaryCorrection::default()
    };
    let minus_one_stop = PrimaryCorrection {
        exposure_milli_stops: -1_000,
        ..PrimaryCorrection::default()
    };
    let doubled = plus_one_stop
        .apply_checked(CHROMA_INPUT)
        .expect("+1 stop exposure");
    let halved = minus_one_stop
        .apply_checked(CHROMA_INPUT)
        .expect("-1 stop exposure");
    for channel in 0..3 {
        assert_matches_spec_f64(
            doubled[channel],
            f64::from(CHROMA_INPUT[channel]) * 2.0,
            "exposure +1000 milli-stops must double linear light",
        );
        assert_matches_spec_f64(
            halved[channel],
            f64::from(CHROMA_INPUT[channel]) / 2.0,
            "exposure -1000 milli-stops must halve linear light",
        );
    }

    // Saturation: -100 is monochrome, and the monochrome value is the Rec.709
    // luma of the input.
    let monochrome = PrimaryCorrection {
        saturation_percent: -100,
        ..PrimaryCorrection::default()
    };
    for input in [TONAL_INPUT, CHROMA_INPUT, [1.0, 0.2, 0.05]] {
        let output = monochrome.apply_checked(input).expect("saturation -100");
        let luma = spec_luma_f64(input.map(f64::from));
        for (channel, value) in output.into_iter().enumerate() {
            assert_matches_spec_f64(
                value,
                luma,
                &format!("saturation -100 on {input:?} channel {channel}"),
            );
        }
    }
    // Saturation 0 is an identity, which §6.1.3 requires alongside -100.
    let saturation_identity = PrimaryCorrection {
        saturation_percent: 0,
        ..PrimaryCorrection::default()
    };
    let identity = saturation_identity
        .apply_checked(CHROMA_INPUT)
        .expect("saturation 0 identity");
    for channel in 0..3 {
        assert_matches_spec_f64(
            identity[channel],
            f64::from(CHROMA_INPUT[channel]),
            "saturation 0 must be an identity",
        );
    }

    // Contrast: the pivot value is preserved for every contrast and every
    // pivot, which is the property that makes the pivot meaningful.
    for pivot_basis_points in [0_i32, 2_500, 4_200, 5_000, 10_000] {
        for contrast_percent in [-100_i32, -35, 35, 100] {
            let correction = PrimaryCorrection {
                contrast_percent,
                contrast_pivot_basis_points: pivot_basis_points,
                ..PrimaryCorrection::default()
            };
            let pivot = f64::from(pivot_basis_points) / 10_000.0;
            let output = correction
                .apply_checked([pivot as f32; 3])
                .expect("contrast pivot");
            for channel in output {
                assert_matches_spec_f64(
                    channel,
                    pivot,
                    &format!("contrast {contrast_percent} must preserve pivot {pivot}"),
                );
            }
        }
    }
    // A non-pivot value must actually move, otherwise "pivot preserved" is a
    // statement about an identity transform.
    let contrast = PrimaryCorrection {
        contrast_percent: 100,
        contrast_pivot_basis_points: 5_000,
        ..PrimaryCorrection::default()
    };
    let stretched = contrast.apply_checked([0.25, 0.5, 0.75]).expect("contrast");
    assert_matches_spec_f64(stretched[0], 0.0, "contrast +100 around 0.5 at 0.25");
    assert_matches_spec_f64(stretched[1], 0.5, "contrast +100 around 0.5 at the pivot");
    assert_matches_spec_f64(stretched[2], 1.0, "contrast +100 around 0.5 at 0.75");

    // Tonal balance: the documented lift magnitudes at their weight maxima.
    let blacks = PrimaryCorrection {
        blacks_percent: 100,
        ..PrimaryCorrection::default()
    };
    assert_matches_spec_f64(
        blacks.apply_checked([0.0; 3]).expect("blacks")[0],
        0.25,
        "blacks +100 lifts a 0.0 channel by the documented 0.25 linear units",
    );
    let whites = PrimaryCorrection {
        whites_percent: 100,
        ..PrimaryCorrection::default()
    };
    assert_matches_spec_f64(
        whites.apply_checked([1.0; 3]).expect("whites")[0],
        1.25,
        "whites +100 lifts a 1.0 channel by the documented 0.25 linear units",
    );
    let shadows = PrimaryCorrection {
        shadows_percent: 100,
        ..PrimaryCorrection::default()
    };
    assert_matches_spec_f64(
        shadows.apply_checked([0.0; 3]).expect("shadows")[0],
        0.20,
        "shadows +100 lifts a 0.0 channel by the documented 0.20 linear units",
    );
    let highlights = PrimaryCorrection {
        highlights_percent: 100,
        ..PrimaryCorrection::default()
    };
    assert_matches_spec_f64(
        highlights.apply_checked([1.0; 3]).expect("highlights")[0],
        1.20,
        "highlights +100 lifts a saturated channel by the documented 0.20 linear units",
    );
    // The weights are computed from the clamped u but applied to the unclamped
    // x, so an over-range channel keeps its over-range value plus the lift.
    assert_matches_spec_f64(
        highlights
            .apply_checked([2.5; 3])
            .expect("over-range highlights")[0],
        2.70,
        "tonal weights use clamped u but must not clamp x",
    );

    // White balance: the documented 10% diagonal gains, non-negative at the
    // bounds.
    for signed in [100_i64, -100] {
        let temperature = chart_correction(PrimaryParameter::TemperaturePercent, signed);
        let output = temperature.apply_checked([0.5; 3]).expect("temperature");
        let gain = 0.1 * (signed as f64) / 100.0;
        // The documented diagonal gains applied around unity.
        let raised_gain = 1.0 + gain;
        let lowered_gain = 1.0 - gain;
        let up = 0.5 * raised_gain;
        let down = 0.5 * lowered_gain;
        assert_matches_spec_f64(output[0], up, "temperature red gain");
        assert_matches_spec_f64(output[1], 0.5, "temperature must not touch green");
        assert_matches_spec_f64(output[2], down, "temperature blue gain");
        assert!(output.iter().all(|value| *value >= 0.0));
        let tint = chart_correction(PrimaryParameter::TintPercent, signed);
        let output = tint.apply_checked([0.5; 3]).expect("tint");
        assert_matches_spec_f64(output[0], 0.5, "tint must not touch red");
        assert_matches_spec_f64(output[1], down, "tint green gain");
        assert_matches_spec_f64(output[2], 0.5, "tint must not touch blue");
        assert!(output.iter().all(|value| *value >= 0.0));
    }

    emit_evidence(
        "cc1_primary_controls",
        "cpu_reference",
        Some(ColorSourceProfile::Rec709Video),
        Some(16),
        json!(evidence),
        (3, 1),
        None,
        output_hash(
            serde_json::to_string(&evidence)
                .expect("control evidence")
                .as_bytes(),
        ),
        json!({
            "control_count": 10,
            "tonal_patch": TONAL_INPUT,
            "chroma_patch": CHROMA_INPUT,
            "expectation_source": "cc1 3.2 equations evaluated in f64 inside the fixture",
            "spec_tolerance": SPEC_F64_TOLERANCE,
            "exposure_plus_one_stop_doubles_linear": true,
            "saturation_minus_100_equals_rec709_luma": true,
            "contrast_pivot_preserved_at_every_pivot": true,
        }),
    );
}

fn assert_gpu_parity(gpu: &FixtureGpu) {
    let backend = gpu.backend().to_owned();
    let compositor = Compositor::new(gpu.context());
    let (width, height, frame) = representative_frame();
    let mut cases = Vec::new();
    let mut output_bytes = Vec::new();
    let representative = representative_correction();
    let (metric, linear_metric, actual) = assert_gpu_control_case(
        &compositor,
        width,
        height,
        &frame,
        representative,
        OverRangeBand::Required,
    );
    let representative_luma = monitor_luma_and_clipping(&actual);
    output_bytes.extend_from_slice(&actual);
    cases.push(json!({
        "case": "representative_all_controls",
        "controls": correction_value_json(representative),
        "monitor_max_code_error": metric.max,
        "monitor_p99_code_error": metric.p99,
        "monitor_mean_code_error": metric.mean,
        "linear": linear_metric.as_json(),
        "over_range_band": OverRangeBand::Required.as_json(),
        "monitor_luma_and_clipping": representative_luma.clone(),
    }));
    for parameter in PrimaryParameter::ALL {
        for value in control_fixture_values(parameter) {
            let parameters = control_case_parameters(parameter, value);
            let effect = effect_with_parameters(10_000, parameters.clone());
            let correction = PrimaryCorrection::from_effect(&effect).expect("GPU control fixture");
            let over_range = representative_over_range_expectation(correction);
            let (metric, linear_metric, actual) =
                assert_gpu_control_case(&compositor, width, height, &frame, correction, over_range);
            let luma = monitor_luma_and_clipping(&actual);
            output_bytes.extend_from_slice(&actual);
            cases.push(json!({
                "case": "single_control",
                "parameter": parameter.name(),
                "value": value,
                "case_parameters": parameters
                    .iter()
                    .map(|(name, value)| ((*name).to_owned(), Value::from(*value)))
                    .collect::<serde_json::Map<_, _>>(),
                "monitor_max_code_error": metric.max,
                "monitor_p99_code_error": metric.p99,
                "monitor_mean_code_error": metric.mean,
                "linear": linear_metric.as_json(),
                "over_range_band": over_range.as_json(),
                "monitor_luma_and_clipping": luma,
            }));
        }
    }
    emit_evidence(
        "cc1_gpu_cpu_parity",
        &backend,
        Some(ColorSourceProfile::Rec709Video),
        Some(16),
        json!({"primary_correction": "representative_plus_every_control_boundary"}),
        (width, height),
        None,
        output_hash(&output_bytes),
        json!({
            "lane": gpu.lane.id(),
            "linear_storage": "rgba16float",
            "representative_monitor_luma_and_clipping": representative_luma,
            "cases": cases,
            "control_case_count": 1 + PrimaryParameter::ALL.iter().map(|parameter| control_fixture_values(*parameter).len()).sum::<usize>(),
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

#[test]
fn cc1_gpu_compositor_matches_canonical_cpu_reference_on_software_fallback() {
    assert_gpu_parity(&fallback_gpu());
}

#[test]
#[ignore = "requires a physical supported GPU; run explicitly with --ignored --nocapture"]
fn cc1_gpu_compositor_matches_canonical_cpu_reference_on_hardware() {
    assert_gpu_parity(&hardware_gpu());
}

#[test]
fn cc1_no_intermediate_clamp_preserves_recoverable_over_range_values() {
    let positive = PrimaryCorrection {
        exposure_milli_stops: 1_000,
        ..PrimaryCorrection::default()
    };
    let negative = PrimaryCorrection {
        exposure_milli_stops: -1_000,
        ..PrimaryCorrection::default()
    };
    let input = [0.75, 0.5, 0.25];
    let over_range = positive.apply_checked(input).expect("positive exposure");
    assert!(over_range.iter().all(|value| *value > 0.0));
    assert!(over_range[0] > 1.0);
    let recovered = apply_primary_corrections(input, &[positive, negative]).expect("recovery");
    for (actual, expected) in recovered.iter().copied().zip(input) {
        assert!((actual - expected).abs() <= 1.0e-6);
    }
    // The negative control: what a display-range clamp *between* the two nodes
    // would produce. This must be materially different from the managed
    // result, otherwise the fixture is comparing the pipeline with itself.
    let clamped_between_nodes = negative
        .apply_checked(over_range.map(|value| value.clamp(0.0, 1.0)))
        .expect("clamped recovery");
    assert_eq!(
        clamped_between_nodes[0], 0.5,
        "clamping 1.5 to 1.0 and halving it must land on 0.5"
    );
    let clamped_recovery_differs = clamped_between_nodes
        .iter()
        .zip(recovered)
        .any(|(clamped, managed)| (clamped - managed).abs() > 1.0e-3);
    assert!(
        clamped_recovery_differs,
        "the clamped control produced the managed result: clamped={clamped_between_nodes:?} managed={recovered:?}"
    );
    assert!(
        (clamped_between_nodes[0] - recovered[0]).abs() > 0.24,
        "the recoverable highlight must be visibly lost by an intermediate clamp"
    );

    let gpu = fallback_gpu();
    let compositor = Compositor::new(gpu.context());
    let frame = working_frame(1, 1, &[input]);
    let effect_positive = correction_effect(1, positive);
    let effect_negative = correction_effect(2, negative);
    let working = compositor
        .render_working(
            (1, 1),
            &[CompositorLayer {
                frame: &frame,
                effects: &[effect_positive, effect_negative],
                transition: TransitionRenderParams::default(),
            }],
        )
        .expect("production WGSL no-intermediate-clamp readback");
    assert_eq!(working.len(), 4);
    for (actual, expected) in working[..3].iter().copied().zip(input) {
        assert!(
            (actual - expected).abs() <= NO_CLAMP_RECOVERY_MAX,
            "production WGSL clamped or failed recovery: actual={actual} expected={expected}"
        );
    }

    // §6.1.4 asks for an over-range *ramp*, not one pixel: a single texel can
    // pass by accident, and a clamp at any stage shows up as a plateau.
    let ramp_width = 64_u32;
    let ramp_height = 4_u32;
    let ramp_top = 4.0_f32;
    let ramp_values = (0..ramp_width)
        .map(|column| ramp_top * column as f32 / (ramp_width - 1) as f32)
        .collect::<Vec<_>>();
    let ramp_rgb = (0..ramp_width * ramp_height)
        .map(|index| {
            let value = ramp_values[(index % ramp_width) as usize];
            [value, value * 0.5, value * 0.25]
        })
        .collect::<Vec<_>>();
    let ramp_frame = working_frame(ramp_width, ramp_height, &ramp_rgb);
    let minus_two_stops = PrimaryCorrection {
        exposure_milli_stops: -2_000,
        ..PrimaryCorrection::default()
    };
    let ramp_working = compositor
        .render_working(
            (ramp_width, ramp_height),
            &[CompositorLayer {
                frame: &ramp_frame,
                effects: &[correction_effect(3, minus_two_stops)],
                transition: TransitionRenderParams::default(),
            }],
        )
        .expect("production WGSL over-range ramp readback");
    let expected_ramp = ramp_rgb
        .iter()
        .flat_map(|rgb| {
            let output = minus_two_stops
                .apply_checked(*rgb)
                .expect("-2 stops on the over-range ramp");
            output
                .into_iter()
                .map(|value| f16::from_f32(value).to_f32())
                .chain(std::iter::once(1.0))
        })
        .collect::<Vec<_>>();
    let ramp_metric = linear_parity_metrics(&ramp_working, &expected_ramp);
    assert_linear_parity(&ramp_metric, "over-range ramp at -2 stops");
    // A clamp anywhere before the exposure node would cap every over-range
    // input at 1.0, so every sample above 1.0 would collapse onto 0.25.
    let mut over_range_samples = 0_u32;
    for (index, pixel) in ramp_working.as_chunks::<4>().0.iter().enumerate() {
        let source = ramp_rgb[index][0];
        if source <= 1.0 {
            continue;
        }
        over_range_samples += 1;
        assert!(
            pixel[0] > 0.25 + 1.0e-3,
            "over-range ramp sample {index} (source {source}) collapsed to a clamped {}",
            pixel[0]
        );
        assert!(
            (pixel[0] - source * 0.25).abs() <= LINEAR_CPU_GPU_MAX,
            "over-range ramp sample {index} lost value: actual={} expected={}",
            pixel[0],
            source * 0.25
        );
    }
    assert!(
        over_range_samples >= ramp_width * ramp_height / 4,
        "the no-clamp ramp must contain a substantial over-range region: {over_range_samples}"
    );

    let recovered_hash = bytemuck_free_f32_bytes(&recovered);
    emit_evidence(
        "cc1_no_intermediate_clamp",
        gpu.backend(),
        Some(ColorSourceProfile::Rec709Video),
        Some(16),
        json!({"nodes": ["exposure:+1", "exposure:-1"], "ramp_nodes": ["exposure:-2"]}),
        (ramp_width, ramp_height),
        None,
        output_hash(&recovered_hash),
        json!({
            "lane": gpu.lane.id(),
            "over_range_before_clamp": over_range,
            "recovered": recovered,
            "clamped_between_nodes": clamped_between_nodes,
            "clamped_recovery_differs": clamped_recovery_differs,
            "recovery_gate_max": NO_CLAMP_RECOVERY_MAX,
            "production_working": working[..3].to_vec(),
            "over_range_ramp_top": ramp_top,
            "over_range_ramp_samples": over_range_samples,
            "over_range_ramp_linear": ramp_metric.as_json(),
        }),
    );
}

fn bytemuck_free_f32_bytes(values: &[f32; 3]) -> Vec<u8> {
    values
        .iter()
        .flat_map(|value| value.to_le_bytes())
        .collect()
}

/// Assert the complete typed status surface for one classifier failure.
///
/// §2.1 requires the error to name the asset field, the observed value, and
/// the allowed values; asserting only `field()` lets the other three drift.
fn assert_typed_source_error(
    error: &kinewright_core::ColorSourceError,
    code: &str,
    field: &str,
    observed: &str,
    allowed: &str,
) {
    assert_eq!(error.code(), code, "code for {error}");
    assert_eq!(error.field(), field, "field for {error}");
    assert_eq!(error.observed(), observed, "observed for {error}");
    assert_eq!(
        error.allowed_values(),
        allowed,
        "allowed values for {error}"
    );
    let message = error.actionable_message();
    for fragment in [field, observed, allowed] {
        assert!(
            message.contains(fragment),
            "actionable message for {error} omitted {fragment:?}: {message}"
        );
    }
    assert!(
        message.contains("Apply an explicit supported source-colour override"),
        "actionable message for {error} omitted the recovery action: {message}"
    );
}

#[test]
fn cc1_source_profile_classification_is_typed_and_actionable() {
    // §2.1 allowed-value strings, asserted verbatim so a silently widened
    // profile table fails here rather than in production.
    const PRIMARIES_ALLOWED: &str = "bt709 or srgb in a supported CC1 profile";
    const TRANSFER_ALLOWED: &str = "bt709, bt1886, or srgb in a matching profile";
    const MATRIX_ALLOWED: &str = "bt709/rgb or rgb/identity in a matching profile";
    const DEPTH_ALLOWED: &str = "integer depth 8..=16";
    const WHITE_POINT_ALLOWED: &str = "d65, or an explicit D65 assumption for BT.709";

    initialize_ffmpeg().expect("FFmpeg must initialize for the source-classification fixture");
    let directory = TempDirectory::new("cc1-source-classification");
    let (actual_path, _) = generate_delivery_source(&directory, 32, 16);
    let supported = rec709_description(8, ColorRange::Limited, ColorTransfer::Bt709);
    assert_eq!(
        classify_source(&supported),
        Ok(ColorSourceProfile::Rec709Video)
    );

    // §2.1: `ColorBitDepth::Integer(n)` and the named variants are equivalent,
    // so a 10-bit source must classify identically either way.
    let mut named_ten = rec709_description(8, ColorRange::Limited, ColorTransfer::Bt709);
    named_ten.bit_depth = ColorBitDepth::Ten;
    let mut numeric_ten = named_ten.clone();
    numeric_ten.bit_depth = ColorBitDepth::integer(10);
    assert_eq!(numeric_ten.bit_depth, ColorBitDepth::Ten);
    assert_eq!(classify_source(&named_ten), classify_source(&numeric_ten));
    assert_eq!(
        classify_source(&numeric_ten),
        Ok(ColorSourceProfile::Rec709Video)
    );
    assert_eq!(ColorBitDepth::integer(8), ColorBitDepth::Eight);

    let mut typed_cases = Vec::new();
    let mut assert_case = |mutate: &dyn Fn(&mut ColorDescription),
                           code: &str,
                           field: &str,
                           observed: &str,
                           allowed: &str| {
        let mut description = rec709_description(8, ColorRange::Limited, ColorTransfer::Bt709);
        mutate(&mut description);
        let error = classify_source(&description)
            .expect_err("an unsupported CC1 source description must be rejected");
        assert_typed_source_error(&error, code, field, observed, allowed);
        typed_cases.push(json!({
            "code": error.code(),
            "field": error.field(),
            "observed": error.observed(),
            "allowed_values": error.allowed_values(),
            "actionable_message": error.actionable_message(),
        }));
    };

    assert_case(
        &|description| description.primaries = ColorPrimaries::Bt2020,
        "unsupported_source_primaries",
        "primaries",
        "Bt2020",
        PRIMARIES_ALLOWED,
    );
    assert_case(
        &|description| description.primaries = ColorPrimaries::DisplayP3,
        "unsupported_source_primaries",
        "primaries",
        "DisplayP3",
        PRIMARIES_ALLOWED,
    );
    assert_case(
        &|description| description.primaries = ColorPrimaries::DciP3,
        "unsupported_source_primaries",
        "primaries",
        "DciP3",
        PRIMARIES_ALLOWED,
    );
    assert_case(
        &|description| description.transfer = ColorTransfer::Smpte2084,
        "unsupported_source_transfer",
        "transfer",
        "Smpte2084",
        TRANSFER_ALLOWED,
    );
    assert_case(
        &|description| description.transfer = ColorTransfer::AribStdB67,
        "unsupported_source_transfer",
        "transfer",
        "AribStdB67",
        TRANSFER_ALLOWED,
    );
    assert_case(
        &|description| description.transfer = ColorTransfer::Log,
        "unsupported_source_transfer",
        "transfer",
        "Log",
        TRANSFER_ALLOWED,
    );
    assert_case(
        &|description| description.transfer = ColorTransfer::LogC,
        "unsupported_source_transfer",
        "transfer",
        "LogC",
        TRANSFER_ALLOWED,
    );
    assert_case(
        &|description| description.transfer = ColorTransfer::Log3G10,
        "unsupported_source_transfer",
        "transfer",
        "Log3G10",
        TRANSFER_ALLOWED,
    );
    assert_case(
        &|description| description.matrix = ColorMatrix::Smpte170M,
        "unsupported_source_matrix",
        "matrix",
        "Smpte170M",
        MATRIX_ALLOWED,
    );
    assert_case(
        &|description| description.bit_depth = ColorBitDepth::Float16,
        "unsupported_source_bit_depth",
        "bit_depth",
        "Float16",
        DEPTH_ALLOWED,
    );
    assert_case(
        &|description| description.bit_depth = ColorBitDepth::integer(17),
        "unsupported_source_bit_depth",
        "bit_depth",
        "Integer(17)",
        DEPTH_ALLOWED,
    );
    assert_case(
        &|description| description.bit_depth = ColorBitDepth::integer(7),
        "unsupported_source_bit_depth",
        "bit_depth",
        "Integer(7)",
        DEPTH_ALLOWED,
    );
    assert_case(
        &|description| description.bit_depth = ColorBitDepth::Unknown,
        "unknown_source_bit_depth",
        "bit_depth",
        "unknown",
        DEPTH_ALLOWED,
    );
    assert_case(
        &|description| description.white_point = ColorWhitePoint::Unknown,
        "unknown_source_white_point",
        "white_point",
        "unknown",
        WHITE_POINT_ALLOWED,
    );
    assert_case(
        &|description| description.primaries = ColorPrimaries::Unknown,
        "unknown_source_primaries",
        "primaries",
        "unknown",
        PRIMARIES_ALLOWED,
    );
    assert_case(
        &|description| description.transfer = ColorTransfer::Unknown,
        "unknown_source_transfer",
        "transfer",
        "unknown",
        TRANSFER_ALLOWED,
    );
    assert_eq!(typed_cases.len(), 16);

    // An explicit D65 assumption is the documented recovery for an unknown
    // BT.709 white point, and it must not rewrite the raw metadata.
    let mut unknown_white_point = rec709_description(8, ColorRange::Limited, ColorTransfer::Bt709);
    unknown_white_point.white_point = ColorWhitePoint::Unknown;
    assert_eq!(
        classify_source_with_assumption(
            &unknown_white_point,
            Some(ColorSourceProfileAssumption::D65)
        ),
        Ok(ColorSourceProfile::Rec709Video)
    );
    assert_eq!(unknown_white_point.white_point, ColorWhitePoint::Unknown);

    // Entirely unknown and partial metadata block the managed decoder, not
    // just the classifier.
    let unknown_description = ColorDescription::unknown();
    let unknown_error = classify_source(&unknown_description)
        .expect_err("unknown source metadata must block managed classification");
    assert_typed_source_error(
        &unknown_error,
        "unknown_source_primaries",
        "primaries",
        "unknown",
        PRIMARIES_ALLOWED,
    );
    let unknown_decode_error = match VideoDecoder::open_scaled_managed(
        &actual_path,
        Rational::new(1, 1).expect("one fps"),
        None,
        &unknown_description,
        None,
    ) {
        Ok(_) => panic!("unknown source metadata must block managed decode"),
        Err(error) => error,
    };
    assert!(matches!(unknown_decode_error, MediaError::Backend(_)));

    let mut partial_description = rec709_description(8, ColorRange::Limited, ColorTransfer::Bt709);
    partial_description.bit_depth = ColorBitDepth::Unknown;
    partial_description.confidence_basis_points = 2_000;
    partial_description.provenance = ColorProvenance::StreamMetadata;
    let partial_error = classify_source(&partial_description)
        .expect_err("partial source metadata must block managed classification");
    assert_typed_source_error(
        &partial_error,
        "unknown_source_bit_depth",
        "bit_depth",
        "unknown",
        DEPTH_ALLOWED,
    );
    let partial_decode_error = match VideoDecoder::open_scaled_managed(
        &actual_path,
        Rational::new(1, 1).expect("one fps"),
        None,
        &partial_description,
        None,
    ) {
        Ok(_) => panic!("partial source metadata must block managed decode"),
        Err(error) => error,
    };
    assert!(matches!(partial_decode_error, MediaError::Backend(_)));

    // High confidence does not make an unsupported tuple supported.
    let mut confident_hlg = rec709_description(8, ColorRange::Limited, ColorTransfer::Bt709);
    confident_hlg.transfer = ColorTransfer::AribStdB67;
    confident_hlg.confidence_basis_points = 10_000;
    confident_hlg.provenance = ColorProvenance::UserOverride;
    assert_eq!(
        classify_source(&confident_hlg)
            .expect_err("a confident HLG override is still unsupported")
            .code(),
        "unsupported_source_transfer"
    );

    let mut decoder_cases = Vec::new();
    for transfer in [
        ColorTransfer::AribStdB67,
        ColorTransfer::Log,
        ColorTransfer::LogC,
        ColorTransfer::Log3G10,
        ColorTransfer::Smpte2084,
    ] {
        let mut description = rec709_description(8, ColorRange::Limited, ColorTransfer::Bt709);
        description.transfer = transfer.clone();
        let error = classify_source(&description).expect_err("unsupported transfer");
        let decoder_error = match VideoDecoder::open_scaled_managed(
            &actual_path,
            Rational::new(1, 1).expect("one fps"),
            None,
            &description,
            Some(ColorSourceProfileAssumption::D65),
        ) {
            Ok(_) => panic!("actual source with unsupported transfer must block managed decode"),
            Err(error) => error,
        };
        assert!(matches!(decoder_error, MediaError::Backend(_)));
        decoder_cases.push(json!({
            "transfer": transfer,
            "code": error.code(),
            "field": error.field(),
            "observed": error.observed(),
            "allowed_values": error.allowed_values(),
            "managed_decode_blocked": true,
        }));
    }
    for depth in [ColorBitDepth::integer(7), ColorBitDepth::integer(17)] {
        let mut description = rec709_description(8, ColorRange::Limited, ColorTransfer::Bt709);
        description.bit_depth = depth.clone();
        let decoder_error = match VideoDecoder::open_scaled_managed(
            &actual_path,
            Rational::new(1, 1).expect("one fps"),
            None,
            &description,
            Some(ColorSourceProfileAssumption::D65),
        ) {
            Ok(_) => panic!("unsupported integer depth must block managed decode"),
            Err(error) => error,
        };
        assert!(matches!(decoder_error, MediaError::Backend(_)));
        decoder_cases.push(json!({
            "bit_depth": depth,
            "managed_decode_blocked": true,
            "error": decoder_error.to_string(),
        }));
    }

    // The delivery QA report must also refuse an unsupported source.
    let mut blocked_asset = probe_path(&actual_path, AssetId(7)).expect("actual source probe");
    blocked_asset.color_description.transfer = ColorTransfer::AribStdB67;
    let blocked_document = simple_document(blocked_asset, (32, 16));
    let conformance =
        delivery_conformance(&blocked_document, DeliveryProfile::SourceMaster, 50, 50)
            .expect("unsupported source should produce a blocking QA report");
    assert!(!conformance.export_ready());
    assert!(conformance.issues.iter().any(|issue| {
        issue.severity == kinewright_core::QaSeverity::Error && issue.code.contains("source")
    }));

    emit_evidence(
        "cc1_unsupported_metadata_classification",
        "cpu_reference",
        None,
        None,
        json!({"supported": ["rec709_video", "srgb_full"], "recovery": "explicit_source_override_or_relink"}),
        (0, 0),
        None,
        output_hash(unknown_error.actionable_message().as_bytes()),
        json!({
            "typed_cases": typed_cases,
            "managed_decode_cases": decoder_cases,
            "integer_depth_equivalence": {"integer_10_equals_ten": true, "integer_8_equals_eight": true},
            "unknown_metadata": {"code": unknown_error.code(), "managed_decode_blocked": true, "error": unknown_decode_error.to_string()},
            "partial_metadata": {"code": partial_error.code(), "managed_decode_blocked": true, "error": partial_decode_error.to_string()},
            "d65_assumption_preserves_raw_metadata": true,
            "delivery_conformance_export_ready": conformance.export_ready(),
        }),
    );
}

#[test]
fn cc1_unsupported_source_blocks_managed_proof_and_export() {
    initialize_ffmpeg().expect("FFmpeg must initialize for the unsupported-proof fixture");
    let directory = TempDirectory::new("cc1-unsupported-proof");
    let (actual_path, _) = generate_delivery_source(&directory, 32, 16);
    let gpu = fallback_gpu();

    let mut blocked_asset = probe_path(&actual_path, AssetId(7)).expect("actual source probe");
    blocked_asset.color_description.transfer = ColorTransfer::AribStdB67;
    let blocked_document = simple_document(blocked_asset.clone(), (32, 16));

    let engine = crate::engine::FfmpegMediaEngine::new_with_gpu(gpu.context())
        .expect("media engine should start for unsupported proof gate");
    let proof_error = match engine
        .monitor_proof_for_document(Arc::new(blocked_document.clone()), TimeCode::ZERO)
    {
        Ok(_) => panic!("unsupported source must block production full-raster proof"),
        Err(error) => error,
    };
    assert!(
        proof_error
            .to_string()
            .contains("managed source profile rejected"),
        "unexpected proof block: {proof_error}"
    );

    let blocked_export_path = directory.path("cc1-blocked-export.mp4");
    let blocked_settings = DeliveryProfile::SourceMaster
        .export_settings(&blocked_document, ExportCancellation::default());
    let (progress_tx, _progress_rx) = crossbeam_channel::unbounded();
    let export_error = match crate::export::export_document(
        &blocked_document,
        &blocked_export_path,
        &blocked_settings,
        &progress_tx,
        gpu.context(),
    ) {
        Ok(()) => panic!("unsupported source must block production export"),
        Err(error) => error,
    };
    assert!(!blocked_export_path.exists());

    // §2.1: a *user override* that still does not match a supported profile is
    // an explicit failure too, and it must travel through the ordinary Core
    // operation path rather than a fixture-local mutation.
    let mut override_document = simple_document(
        probe_path(&actual_path, AssetId(9)).expect("override source probe"),
        (32, 16),
    );
    override_document.color_context = ColorContext::sdr_rec709();
    let core = Core::spawn(override_document).expect("override core");
    let bt2020_override = ColorDescription {
        primaries: ColorPrimaries::Bt2020,
        transfer: ColorTransfer::Bt709,
        matrix: ColorMatrix::Bt709,
        range: ColorRange::Limited,
        white_point: ColorWhitePoint::D65,
        bit_depth: ColorBitDepth::Eight,
        confidence_basis_points: 10_000,
        provenance: ColorProvenance::UserOverride,
    };
    let overridden = match core
        .request(Command::Do(Operation::SetAssetColorDescription {
            asset: AssetId(9),
            color_description: bt2020_override.clone(),
        }))
        .expect("the override operation itself must be accepted")
    {
        Event::DocumentChanged { doc, .. } => doc,
        other => panic!("override was not applied: {other:?}"),
    };
    assert_eq!(
        overridden
            .asset(AssetId(9))
            .expect("overridden asset")
            .color_description,
        bt2020_override,
        "an explicit override must be stored verbatim, not silently corrected"
    );
    let override_error = classify_source(&bt2020_override)
        .expect_err("a BT.2020 user override is still an unsupported CC1 source");
    assert_eq!(override_error.code(), "unsupported_source_primaries");
    let override_proof_error =
        match engine.monitor_proof_for_document(Arc::new((*overridden).clone()), TimeCode::ZERO) {
            Ok(_) => panic!("an unsupported user override must block the managed proof"),
            Err(error) => error,
        };
    let override_export_path = directory.path("cc1-blocked-override-export.mp4");
    let override_settings =
        DeliveryProfile::SourceMaster.export_settings(&overridden, ExportCancellation::default());
    let override_export_error = match crate::export::export_document(
        &overridden,
        &override_export_path,
        &override_settings,
        &progress_tx,
        gpu.context(),
    ) {
        Ok(()) => panic!("an unsupported user override must block production export"),
        Err(error) => error,
    };
    assert!(!override_export_path.exists());

    emit_evidence(
        "cc1_unsupported_metadata",
        gpu.backend(),
        None,
        None,
        json!({"supported": ["rec709_video", "srgb_full"], "recovery": "explicit_source_override_or_relink"}),
        (32, 16),
        Some(file_hash(&actual_path)),
        output_hash(override_error.actionable_message().as_bytes()),
        json!({
            "lane": gpu.lane.id(),
            "proof_blocked": true,
            "export_blocked": true,
            "proof_block_error": proof_error.to_string(),
            "export_block_error": export_error.to_string(),
            "user_override": {
                "operation": "SetAssetColorDescription",
                "code": override_error.code(),
                "field": override_error.field(),
                "observed": override_error.observed(),
                "allowed_values": override_error.allowed_values(),
                "actionable_message": override_error.actionable_message(),
                "stored_verbatim": true,
                "proof_blocked": override_proof_error.to_string(),
                "export_blocked": override_export_error.to_string(),
            },
            "actual_source": actual_path,
        }),
    );
}

pub(crate) fn simple_document(asset: MediaAsset, resolution: (u32, u32)) -> Document {
    let duration = if asset.duration > TimeCode::ZERO {
        asset.duration
    } else {
        TimeCode(1)
    };
    let fps = asset.fps;
    Document {
        color_context: ColorContext::default(),
        tracks: vec![Track {
            id: TrackId(1),
            kind: TrackKind::Video,
            sync_lock: true,
            clips: vec![Clip {
                id: ClipId(1),
                asset: asset.id,
                source_range: TimeCode::ZERO..duration,
                content: ClipContent::Media,
                timeline_start: TimeCode::ZERO,
                effects: Vec::new(),
                transition_in: None,
                link: None,
                audio_gain_tenth_db: 0,
                audio_fade_in_frames: TimeCode::ZERO,
                audio_fade_out_frames: TimeCode::ZERO,
                speed_percent: 100,
            }],
        }],
        media_pool: vec![asset],
        fps,
        resolution,
        duration,
        ..Document::default()
    }
}

/// The bar luma codes written into the delivery/proof source raster.
const DELIVERY_SOURCE_BAR_CODES: [u8; 5] = [16, 64, 128, 192, 235];

/// The monitor codes those limited-range BT.709 bars must decode to.
///
/// Limited expansion is `E = (Y - 16) / 219`; the BT.709 EOTF and OETF are
/// inverses on that domain, so the expected code is `round(255 * E)`:
/// 0, round(255*48/219)=56, round(255*112/219)=130, round(255*176/219)=205,
/// and 255. §6.2 allows one code of slack at the range endpoints.
const DELIVERY_SOURCE_BAR_MONITOR_CODES: [u8; 5] = [0, 56, 130, 205, 255];

/// Decode the proof/delivery source through the managed decoder so the
/// production GPU raster can be compared against the independent CPU
/// reference rather than only against another GPU raster.
pub(crate) fn decode_managed_working_frame(
    path: &Path,
    description: &ColorDescription,
) -> WorkingFrame {
    let mut decoder = VideoDecoder::open_scaled_managed(
        path,
        Rational::new(1, 1).expect("one fps"),
        None,
        description,
        Some(ColorSourceProfileAssumption::D65),
    )
    .unwrap_or_else(|error| panic!("managed decode failed for {}: {error}", path.display()));
    let mut cache = crate::cache::FrameCache::<WorkingFrame>::new(2);
    decoder
        .decode_window(TimeCode::ZERO, TimeCode::ZERO, &mut cache)
        .unwrap_or_else(|error| panic!("managed frame decode failed: {error}"));
    cache
        .frame_at_or_before(TimeCode::ZERO)
        .expect("managed frame should be cached")
}

/// Assert the five source bars land on their §3.1 monitor codes and that the
/// decoded raster never descends left to right.
fn assert_bar_endpoints_and_monotonicity(rgba: &[u8], width: u32, height: u32) {
    let pixels = rgba.as_chunks::<4>().0;
    assert_eq!(pixels.len(), (width * height) as usize);
    for (bar, expected) in DELIVERY_SOURCE_BAR_MONITOR_CODES.into_iter().enumerate() {
        // Sample the interior of each bar, away from any codec/sampler seam.
        let bar_start = width * bar as u32 / 5;
        let bar_end = width * (bar as u32 + 1) / 5;
        let column = u32::midpoint(bar_start, bar_end);
        let pixel = pixels[column as usize];
        for (channel, value) in pixel[..3].iter().enumerate() {
            assert!(
                value.abs_diff(expected) <= 1,
                "bar {bar} channel {channel} (source luma {}) decoded to {pixel:?}, expected {expected} within 1 code",
                DELIVERY_SOURCE_BAR_CODES[bar]
            );
        }
    }
    for row in 0..height {
        let start = (row * width) as usize;
        for column in 1..width as usize {
            let previous = pixels[start + column - 1];
            let current = pixels[start + column];
            for channel in 0..3 {
                assert!(
                    current[channel] >= previous[channel],
                    "decoded proof ramp descended on row {row} at column {column} channel {channel}: {} -> {}",
                    previous[channel],
                    current[channel]
                );
            }
        }
    }
}

#[test]
fn cc1_full_raster_monitor_proof_has_same_render_semantics_as_monitor_preview() {
    initialize_ffmpeg().expect("FFmpeg must initialize for proof fixture");
    let directory = TempDirectory::new("cc1-monitor-proof");
    let width = 2_048_u32;
    let height = 2_u32;
    let (path, source_bytes) = generate_delivery_source(&directory, width, height);
    let asset = probe_path(&path, AssetId(6)).expect("proof source should probe");
    assert_eq!(asset.resolution, Some((width, height)));
    let raw_description = asset.color_description.clone();
    let document = Arc::new(simple_document(asset, (width, height)));
    document.validate().expect("proof document");
    let gpu = fallback_gpu();
    let engine = crate::engine::FfmpegMediaEngine::new_with_gpu(gpu.context())
        .expect("media engine should start on the fixture adapter");
    let proof = engine
        .monitor_proof_for_document(Arc::clone(&document), TimeCode::ZERO)
        .expect("full-raster monitor proof");
    let mut full_renderer = crate::render::FrameRenderer::new(gpu.context());
    let full_render = full_renderer
        .render(
            &document,
            TimeCode::ZERO,
            (width, height),
            crate::render::RenderScale::FullResolution,
            crate::render::DecodeStrategy::Seek,
        )
        .expect("production full-raster preview render");
    let same_full_pixels = full_render.rgba.as_ref() == proof.image.pixels.as_slice();
    assert!(
        same_full_pixels,
        "monitor proof and full preview pixels diverged"
    );

    // Comparing the proof only against another GPU raster proves the two GPU
    // paths agree, not that either matches the CC1 contract. Gate the proof
    // against the independent CPU reference on the same decoded frame.
    let working = decode_managed_working_frame(&path, &raw_description);
    assert_eq!((working.width, working.height), (width, height));
    let cpu_reference = cpu_reference_monitor(&working, &[]);
    let cpu_gpu_metric = abs_code_diff_rgb(&proof.image.pixels, &cpu_reference);
    assert!(
        cpu_gpu_metric.max <= MONITOR_CPU_GPU_MAX,
        "monitor proof vs CPU reference max: {cpu_gpu_metric:?}"
    );
    assert!(
        cpu_gpu_metric.p99 <= MONITOR_CPU_GPU_P99,
        "monitor proof vs CPU reference P99: {cpu_gpu_metric:?}"
    );
    assert!(
        cpu_gpu_metric.mean <= MONITOR_CPU_GPU_MEAN,
        "monitor proof vs CPU reference mean: {cpu_gpu_metric:?}"
    );
    assert_bar_endpoints_and_monotonicity(&proof.image.pixels, width, height);
    assert_bar_endpoints_and_monotonicity(&cpu_reference, width, height);

    let preview = engine
        .thumbnail_for_document(Arc::clone(&document), TimeCode::ZERO, 512)
        .expect("same-raster monitor preview");
    assert_eq!((proof.image.width, proof.image.height), (width, height));
    assert!(
        preview.width < proof.image.width,
        "thumbnail must be a proxy raster"
    );
    assert!(preview.height <= proof.image.height);
    assert!(!proof.image.pixels.is_empty());
    assert!(!preview.pixels.is_empty());
    assert!(proof.metadata.full_resolution);
    assert!(matches!(
        proof.metadata.render_kind,
        kinewright_core::MonitorProofRenderKind::GpuPreview
    ));
    gpu.assert_proof_provenance(&proof.metadata);
    let hash = output_hash(&proof.image.pixels);
    emit_evidence(
        "cc1_full_raster_monitor_proof",
        gpu.backend(),
        Some(ColorSourceProfile::Rec709Video),
        Some(8),
        json!({
            "primary_correction": "neutral",
            "assumption": "d65",
            "source_description_raw": raw_description,
            "context": ColorContext::sdr_rec709(),
            "revision": git_revision(),
        }),
        (width, height),
        Some(file_hash(&path)),
        hash,
        json!({
            "lane": gpu.lane.id(),
            "proof_raster": [proof.image.width, proof.image.height],
            "preview_raster": [preview.width, preview.height],
            "proxy_smaller": preview.width < proof.image.width,
            "same_render_semantics": same_full_pixels,
            "cpu_reference_max_code_error": cpu_gpu_metric.max,
            "cpu_reference_p99_code_error": cpu_gpu_metric.p99,
            "cpu_reference_mean_code_error": cpu_gpu_metric.mean,
            "range_endpoint_codes": DELIVERY_SOURCE_BAR_MONITOR_CODES,
            "source_bar_luma_codes": DELIVERY_SOURCE_BAR_CODES,
            "monotonic_after_final_encoding": true,
            "source_raw_hash": output_hash(&source_bytes),
            "proof_metadata": proof.metadata,
        }),
    );
}

pub(crate) fn generate_delivery_source(
    directory: &TempDirectory,
    width: u32,
    height: u32,
) -> (PathBuf, Vec<u8>) {
    assert_eq!(width % 2, 0, "delivery fixture width must be even");
    assert_eq!(height % 2, 0, "delivery fixture height must be even");
    let mut y_plane = Vec::with_capacity(usize::try_from(width * height).expect("Y plane"));
    for _y in 0..height {
        for x in 0..width {
            // Wide gray bars keep chroma neutral and avoid using codec loss as
            // a substitute for compositor parity.
            let bar = (x * 5 / width).min(4);
            y_plane.push(DELIVERY_SOURCE_BAR_CODES[usize::try_from(bar).expect("bar")]);
        }
    }
    let chroma_len = usize::try_from(width * height).expect("chroma planes");
    let mut input = y_plane;
    input.extend(std::iter::repeat_n(128_u8, chroma_len));
    input.extend(std::iter::repeat_n(128_u8, chroma_len));
    let path = directory.path("cc1-delivery-source.mkv");
    let size = format!("{width}x{height}");
    let mut command = ProcessCommand::new(ffmpeg_executable());
    command
        .args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-y",
            "-f",
            "rawvideo",
            "-pix_fmt",
            "yuv444p",
            "-s",
            &size,
            "-r",
            "1",
            "-i",
            "pipe:0",
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=440:sample_rate=48000:duration=1",
            "-vf",
            "setparams=range=limited:color_primaries=bt709:color_trc=bt709:colorspace=bt709",
            "-frames:v",
            "1",
            "-c:v",
            "ffv1",
            "-level",
            "3",
            "-g",
            "1",
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
            "-c:a",
            "pcm_s16le",
            "-shortest",
        ])
        .arg(&path)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    let mut child = command
        .spawn()
        .expect("delivery source FFmpeg should start");
    child
        .stdin
        .take()
        .expect("delivery source stdin")
        .write_all(&input)
        .expect("write delivery source raster");
    let output = child
        .wait_with_output()
        .expect("delivery source FFmpeg process");
    assert!(
        output.status.success(),
        "delivery source generation failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    (path, input)
}

/// Round the 16-bit delivery intermediate to 8-bit codes.
///
/// The export path now quantizes once at 16 bits, so the decoded H.264 frame
/// must be compared against *that* contract, not against the RGBA8 monitor
/// raster, which uses a different (monitoring) target.
///
/// This is the exact inverse of the intermediate's scale: `round(255 * v /
/// DELIVERY_INTERMEDIATE_WHITE)`.  Because the intermediate's white is `255 <<
/// 8`, that expression equals `round(v / 256)` for every code the encoder can
/// produce; the general form is written out with integer rounding so no float
/// division can drift, and so a change to the constant is followed here
/// automatically.
fn delivery_frame_to_rgba8(frame: &crate::compositor::DeliveryFrame) -> Vec<u8> {
    let white = u32::from(DELIVERY_INTERMEDIATE_WHITE);
    frame
        .rgba64le
        .as_chunks::<2>()
        .0
        .iter()
        .map(|bytes| {
            let code = u32::from(u16::from_le_bytes(*bytes));
            let rounded = (code * 255 + white / 2) / white;
            u8::try_from(rounded.min(255)).unwrap_or(u8::MAX)
        })
        .collect()
}

#[test]
fn cc1_h264_yuv420p_delivery_is_measured_separately_from_gpu_gate() {
    initialize_ffmpeg().expect("FFmpeg must initialize for delivery fixture");
    let directory = TempDirectory::new("cc1-delivery-parity");
    let width = 32_u32;
    let height = 16_u32;
    let (source_path, source_bytes) = generate_delivery_source(&directory, width, height);
    let source_asset = probe_path(&source_path, AssetId(2)).expect("delivery source should probe");
    assert_eq!(
        source_asset.color_description.primaries,
        ColorPrimaries::Bt709
    );
    assert_eq!(
        source_asset.color_description.transfer,
        ColorTransfer::Bt709
    );
    assert_eq!(source_asset.color_description.matrix, ColorMatrix::Bt709);
    assert_eq!(source_asset.color_description.range, ColorRange::Limited);
    assert_eq!(
        source_asset.color_description.bit_depth,
        ColorBitDepth::Eight
    );
    let mut document = simple_document(source_asset, (width, height));
    // Keep the source spatially low-frequency/neutral for YUV420 parity, but
    // make the export exercise the managed primary node rather than a
    // neutral pass-through.
    document.tracks[0].clips[0].effects = vec![correction_effect(
        90,
        PrimaryCorrection {
            exposure_milli_stops: 100,
            ..PrimaryCorrection::default()
        },
    )];
    document
        .validate()
        .expect("delivery primary document should validate");
    let gpu = fallback_gpu();
    let proof_engine = crate::engine::FfmpegMediaEngine::new_with_gpu(gpu.context())
        .expect("production media engine should start for delivery proof");
    let proof = proof_engine
        .monitor_proof_for_document(Arc::new(document.clone()), TimeCode::ZERO)
        .expect("production full-raster delivery proof");
    assert!(proof.metadata.full_resolution);
    gpu.assert_proof_provenance(&proof.metadata);
    let mut direct_renderer = crate::render::FrameRenderer::new(gpu.context());
    let direct_proof = direct_renderer
        .render(
            &document,
            TimeCode::ZERO,
            (width, height),
            crate::render::RenderScale::FullResolution,
            crate::render::DecodeStrategy::Seek,
        )
        .expect("production FrameRenderer delivery proof");
    assert_eq!(
        direct_proof.rgba.as_ref(),
        proof.image.pixels.as_slice(),
        "monitor proof and direct FrameRenderer delivery raster must be identical"
    );

    // The delivery contract reference: the same production renderer, encoded
    // through the *delivery* description and quantized once at 16 bits.
    let delivery = direct_renderer
        .render_delivery(
            &document,
            TimeCode::ZERO,
            (width, height),
            crate::render::RenderScale::FullResolution,
            crate::render::DecodeStrategy::Seek,
        )
        .expect("production FrameRenderer delivery frame");
    assert_eq!((delivery.width, delivery.height), (width, height));
    assert_eq!(
        delivery.rgba64le.len(),
        (width * height * 8) as usize,
        "the delivery intermediate must be RGBA64LE"
    );
    let delivery_reference = delivery_frame_to_rgba8(&delivery);

    let output_path = directory.path("cc1-production-export.mp4");
    let settings = ExportSettings {
        fps: document.fps,
        resolution: document.resolution,
        delivery_color: ColorContext::sdr_rec709().delivery,
        video_codec: "libx264".to_owned(),
        audio_codec: "aac".to_owned(),
        video_bitrate: 20_000_000,
        audio_bitrate: 192_000,
        cancellation: ExportCancellation::default(),
    };
    let (progress_tx, _progress_rx) = crossbeam_channel::unbounded();
    crate::export::export_document(
        &document,
        &output_path,
        &settings,
        &progress_tx,
        gpu.context(),
    )
    .expect("production export_document should create H.264 delivery");
    let asset = probe_path(&output_path, AssetId(3)).expect("production H.264 should probe");
    assert_eq!(asset.color_description.primaries, ColorPrimaries::Bt709);
    assert_eq!(asset.color_description.transfer, ColorTransfer::Bt709);
    assert_eq!(asset.color_description.matrix, ColorMatrix::Bt709);
    assert_eq!(asset.color_description.range, ColorRange::Limited);
    assert_eq!(asset.color_description.bit_depth, ColorBitDepth::Eight);
    let actual_pixel_format = decoded_video_pixel_format(&output_path);
    assert_eq!(actual_pixel_format, "yuv420p");
    let decoded = ffmpeg_cli_decode_rgba(&output_path, width, height);
    let metric = abs_code_diff_rgb(&decoded, &delivery_reference);

    // §6.2: the codec tolerances measure codec loss only and must never be
    // reused for the compositor or CPU/GPU gate.
    let compositor_gate_reused = DELIVERY_CODEC_MAX == MONITOR_CPU_GPU_MAX
        || DELIVERY_CODEC_P99 == MONITOR_CPU_GPU_P99
        || DELIVERY_CODEC_MEAN == MONITOR_CPU_GPU_MEAN;
    assert!(
        !compositor_gate_reused,
        "the H.264 codec tolerances must be distinct from the compositor CPU/GPU gate"
    );
    assert!(
        metric.max <= DELIVERY_CODEC_MAX,
        "H.264 delivery max metric: {metric:?}"
    );
    assert!(
        metric.p99 <= DELIVERY_CODEC_P99,
        "H.264 delivery P99 metric: {metric:?}"
    );
    assert!(
        metric.mean <= DELIVERY_CODEC_MEAN,
        "H.264 delivery mean metric: {metric:?}"
    );
    emit_evidence(
        "cc1_h264_yuv420p_delivery",
        gpu.backend(),
        Some(ColorSourceProfile::Rec709Video),
        Some(8),
        json!({"delivery": "bt709_limited_8bit", "codec": "h264", "pixel_format": "yuv420p"}),
        (width, height),
        Some(file_hash(&source_path)),
        output_hash(&decoded),
        json!({
            "lane": gpu.lane.id(),
            "max_code_error": metric.max,
            "p99_code_error": metric.p99,
            "mean_code_error": metric.mean,
            "comparison_reference": "FrameRenderer::render_delivery rgba64le rounded to 8 bit",
            "delivery_reference_hash": output_hash(&delivery_reference),
            "explicit_bt709_limited_tags": true,
            "actual_pixel_format": actual_pixel_format,
            "compositor_gate_reused": compositor_gate_reused,
            "codec_gate": {"max": DELIVERY_CODEC_MAX, "p99": DELIVERY_CODEC_P99, "mean": DELIVERY_CODEC_MEAN},
            "production_renderer": "monitor_proof_for_document+FrameRenderer+export_document",
            "same_raster_proof": [proof.image.width, proof.image.height],
            "proof_metadata": proof.metadata,
            "source_hash": file_hash(&source_path),
            "source_raw_hash": output_hash(&source_bytes),
            "delivery_backend": "ffmpeg_h264_yuv420p",
        }),
    );
}

fn decoded_video_pixel_format(path: &Path) -> String {
    let input = crate::ffmpeg::format::input(path).expect("exported video should open");
    let stream = input
        .streams()
        .best(crate::ffmpeg::media::Type::Video)
        .expect("exported video should have a video stream");
    let context = crate::ffmpeg::codec::context::Context::from_parameters(stream.parameters())
        .expect("exported video codec parameters should decode");
    let decoder = context
        .decoder()
        .video()
        .expect("exported video decoder should open");
    decoder
        .format()
        .descriptor()
        .map(crate::ffmpeg::format::pixel::Descriptor::name)
        .unwrap_or("unknown")
        .to_owned()
}

fn ffmpeg_cli_decode_rgba(path: &Path, width: u32, height: u32) -> Vec<u8> {
    let output = ProcessCommand::new(ffmpeg_executable())
        .args(["-hide_banner", "-loglevel", "error", "-i"])
        .arg(path)
        .args([
            "-frames:v",
            "1",
            "-f",
            "rawvideo",
            "-pix_fmt",
            "rgba",
            "pipe:1",
        ])
        .output()
        .expect("FFmpeg delivery decode should start");
    assert!(
        output.status.success(),
        "FFmpeg delivery decode failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        output.stdout.len(),
        usize::try_from(width * height * 4).expect("decoded bytes")
    );
    output.stdout
}

fn generate_cache_source(directory: &TempDirectory) -> PathBuf {
    let path = directory.path("cc1-cache-source.mkv");
    let output = ProcessCommand::new(ffmpeg_executable())
        .args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-y",
            "-f",
            "lavfi",
            "-i",
            "testsrc2=size=1280x720:rate=30:duration=2",
            "-vf",
            "setparams=range=limited:color_primaries=bt709:color_trc=bt709:colorspace=bt709",
            "-c:v",
            "ffv1",
            "-level",
            "3",
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
        ])
        .arg(&path)
        .output()
        .expect("cache source FFmpeg should start");
    assert!(
        output.status.success(),
        "cache source generation failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    path
}

#[test]
fn cc1_managed_cache_memory_bound_is_measured_in_working_bytes() {
    initialize_ffmpeg().expect("FFmpeg must initialize for cache fixture");
    let directory = TempDirectory::new("cc1-cache-bound");
    let source_path = generate_cache_source(&directory);
    let asset = probe_path(&source_path, AssetId(5)).expect("cache source should probe");
    assert_eq!(asset.resolution, Some((1280, 720)));
    let duration = asset.duration.0;
    let document = simple_document(asset, (1280, 720));
    let gpu = fallback_gpu();
    let mut renderer = crate::render::FrameRenderer::new(gpu.context());
    let cache_budget = renderer.cache_budget_bytes();
    let frame_count = duration.clamp(32, 60);
    let started = Instant::now();
    let mut last = crate::derived_cache::CacheStats::default();
    let mut eviction_count = renderer.cache_eviction_count();
    let mut eviction_observed = false;
    let mut render_times_ms = Vec::with_capacity(usize::try_from(frame_count).unwrap_or(0));
    for frame in 0..frame_count {
        let render_started = Instant::now();
        renderer
            .render(
                &document,
                TimeCode(frame),
                (1280, 720),
                crate::render::RenderScale::FullResolution,
                crate::render::DecodeStrategy::Sequential,
            )
            .unwrap_or_else(|error| {
                panic!("cache fixture render at frame {frame} failed: {error}")
            });
        render_times_ms.push(render_started.elapsed().as_secs_f64() * 1_000.0);
        let previous = last;
        let previous_evictions = eviction_count;
        last = renderer.cache_stats();
        eviction_count = renderer.cache_eviction_count();
        // The cache is intentionally bounded in terms of working-surface
        // bytes. Observe an actual eviction counter transition (or a
        // resident-entry reduction); do not infer eviction from a hard-coded
        // evidence flag.
        if frame > 0
            && (last.file_count < previous.file_count || eviction_count > previous_evictions)
        {
            eviction_observed = true;
        }
        assert!(
            usize::try_from(last.bytes).unwrap_or(usize::MAX) <= cache_budget,
            "production cache exceeded CC1 bound: {last:?}"
        );
    }
    let elapsed_ms = started.elapsed().as_secs_f64() * 1_000.0;
    let mut sorted_render_times = render_times_ms.clone();
    sorted_render_times.sort_by(f64::total_cmp);
    let percentile = |fraction: f64| {
        let index = ((sorted_render_times.len() as f64 * fraction).ceil() as usize)
            .saturating_sub(1)
            .min(sorted_render_times.len().saturating_sub(1));
        sorted_render_times.get(index).copied().unwrap_or(0.0)
    };
    let frame_bytes = 1280_usize * 720 * 8;
    let theoretical_frames = cache_budget / frame_bytes;
    assert!(theoretical_frames >= 1);
    assert!(last.file_count > 0);
    assert!(last.file_count <= u64::try_from(theoretical_frames + 1).unwrap_or(u64::MAX));
    assert!(last.file_count < u64::try_from(frame_count).unwrap_or(u64::MAX));
    assert!(
        eviction_observed,
        "production cache never evicted a frame: final stats={last:?}, rendered={frame_count}"
    );
    let evidence = json!({
        "raster": [1280, 720],
        "working_frame_bytes": frame_bytes,
        "rendered_frames": frame_count,
        "resident_frames": last.file_count,
        "resident_bytes": last.bytes,
        "budget_bytes": cache_budget,
        "eviction_count": eviction_count,
        "elapsed_ms": elapsed_ms,
        "render_first_ms": render_times_ms.first().copied().unwrap_or(0.0),
        "render_median_ms": percentile(0.50),
        "render_p99_ms": percentile(0.99),
        "eviction_observed": eviction_observed,
        "source_file_hash_sha256": file_hash(&source_path),
        "source_path": source_path,
    });
    emit_evidence(
        "cc1_managed_cache_memory_bound",
        gpu.backend(),
        None,
        Some(16),
        json!({"working_storage": "rgba16float", "cache_state": "ephemeral_not_project_state"}),
        (1280, 720),
        Some(file_hash(&source_path)),
        output_hash(
            serde_json::to_string(&evidence)
                .expect("cache evidence")
                .as_bytes(),
        ),
        evidence,
    );
}
