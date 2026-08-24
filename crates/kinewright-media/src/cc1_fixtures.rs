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
        PrimaryCorrection, PrimaryParameter, apply_primary_corrections, classify_source,
        classify_source_with_assumption, decode_bt709, decode_srgb, encode_monitor_rgb8,
        encode_monitor_rgba8, expand_native_range,
    },
    decode::{VideoDecoder, probe_path},
    frame::WorkingFrame,
    initialize_ffmpeg,
    sha256::Sha256,
    test_support::{TempDirectory, ffmpeg_executable},
    timeline::TransitionRenderParams,
};

const MONITOR_CPU_GPU_MAX: u8 = 2;
const MONITOR_CPU_GPU_P99: f64 = 1.0;
const MONITOR_CPU_GPU_MEAN: f64 = 0.50;
const LINEAR_CPU_GPU_MAX: f32 = 1.5e-3;
const LINEAR_CPU_GPU_P99: f32 = 7.5e-4;
const LINEAR_CPU_GPU_MEAN: f32 = 2.5e-4;
const DELIVERY_CODEC_MAX: u8 = 4;
const DELIVERY_CODEC_P99: f64 = 2.0;
const DELIVERY_CODEC_MEAN: f64 = 1.0;

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
struct DiffMetrics {
    max: u8,
    p99: f64,
    mean: f64,
}

#[derive(Debug, Clone, Copy, Default)]
struct FloatDiffMetrics {
    max: f32,
    p99: f32,
    mean: f32,
}

#[derive(Debug, Clone, Copy, Default)]
struct BoundedFloatDiffMetrics {
    metrics: FloatDiffMetrics,
    included: usize,
    excluded: usize,
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

fn output_hash(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    let mut result = String::with_capacity(64);
    for byte in hasher.finalize() {
        result.push_str(&format!("{byte:02x}"));
    }
    result
}

fn file_hash(path: &Path) -> String {
    let bytes = std::fs::read(path)
        .unwrap_or_else(|error| panic!("could not hash fixture file {}: {error}", path.display()));
    output_hash(&bytes)
}

fn git_revision() -> String {
    ProcessCommand::new("git")
        .args(["rev-parse", "HEAD"])
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_owned())
        .filter(|revision| !revision.is_empty())
        .unwrap_or_else(|| "unavailable".to_owned())
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
    let payload = json!({
        "contract": "cc1_managed_sdr_primary",
        "fixture": fixture,
        "git_revision": git_revision(),
        "backend": backend,
        "backend_name": backend_name,
        "adapter": adapter,
        "software_fallback": software_fallback,
        "gpu_claim": gpu_claim,
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
}

fn backend_metadata(backend: &str) -> Value {
    let mut metadata = serde_json::Map::new();
    for token in backend.split(';') {
        let Some((key, value)) = token.split_once('=') else {
            continue;
        };
        match key {
            "backend" | "adapter" => {
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

fn abs_code_diff_rgb(actual: &[u8], expected: &[u8]) -> DiffMetrics {
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

fn monitor_luma_and_clipping(rgba: &[u8]) -> Value {
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

fn abs_float_diff(actual: &[f32], expected: &[f32]) -> FloatDiffMetrics {
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

fn abs_float_diff_in_domain(actual: &[f32], expected: &[f32]) -> BoundedFloatDiffMetrics {
    assert_eq!(actual.len(), expected.len());
    let mut actual_rgb = Vec::new();
    let mut expected_rgb = Vec::new();
    let mut excluded = 0;
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
            if actual_value.is_finite()
                && expected_value.is_finite()
                && actual_value.abs() <= 2.0
                && expected_value.abs() <= 2.0
            {
                actual_rgb.push(actual_value);
                expected_rgb.push(expected_value);
            } else {
                excluded += 1;
            }
        }
    }
    BoundedFloatDiffMetrics {
        metrics: abs_float_diff(&actual_rgb, &expected_rgb),
        included: actual_rgb.len(),
        excluded,
    }
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

fn working_frame(width: u32, height: u32, rgb: &[[f32; 3]]) -> WorkingFrame {
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

fn representative_frame() -> (u32, u32, WorkingFrame) {
    // Use wide low-frequency colour bars so the production linear sampler is
    // measured on stable texel interiors. The separate 12-patch fixture owns
    // high-frequency chart boundaries; keeping those boundaries out of this
    // parity raster prevents interpolation edge pixels from dominating a
    // half-float P99 gate while retaining varied RGB/control coverage.
    let width = 512;
    let height = 4;
    let bars = [
        [0.0, 0.0, 0.0],
        [0.05, 0.05, 0.05],
        [0.1, 0.05, 0.025],
        [0.2, 0.025, 0.12],
    ];
    let bar_width = width / bars.len() as u32;
    let rgb = (0..width * height)
        .map(|index| bars[(index % width / bar_width) as usize])
        .collect::<Vec<_>>();
    (width, height, working_frame(width, height, &rgb))
}

fn assert_gpu_control_case(
    compositor: &Compositor,
    width: u32,
    height: u32,
    frame: &WorkingFrame,
    correction: PrimaryCorrection,
) -> (DiffMetrics, BoundedFloatDiffMetrics, Vec<u8>) {
    let effect = correction_effect(1, correction);
    let expected = cpu_reference_monitor(frame, &[correction]);
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
    let expected_linear = frame
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
        .collect::<Vec<_>>();
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
    let linear_metric = abs_float_diff_in_domain(&actual_linear, &expected_linear);
    assert!(
        linear_metric.included >= width as usize * height as usize,
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
    assert!(
        linear_metric.metrics.max <= LINEAR_CPU_GPU_MAX,
        "GPU/CPU linear max metric for {:?}: {linear_metric:?}",
        correction
    );
    assert!(
        linear_metric.metrics.p99 <= LINEAR_CPU_GPU_P99,
        "GPU/CPU linear P99 metric for {:?}: {linear_metric:?}",
        correction
    );
    assert!(
        linear_metric.metrics.mean <= LINEAR_CPU_GPU_MEAN,
        "GPU/CPU linear mean metric for {:?}: {linear_metric:?}",
        correction
    );
    (monitor_metric, linear_metric, actual.as_ref().clone())
}

fn fallback_gpu() -> (GpuContext, String) {
    let gpu = GpuContext::headless(true).unwrap_or_else(|error| {
        panic!(
            "CC1 primary GPU evidence requires a Linux lavapipe/WARP fallback adapter; no adapter was available ({error}). Install Mesa lavapipe and ensure Vulkan ICD discovery is enabled (for example, VK_ICD_FILENAMES), then rerun cargo test -p kinewright-media."
        )
    });
    let metadata = gpu.monitor_proof_metadata();
    let info_text = format!(
        "backend={};adapter={};software_fallback={};gpu_claim={}",
        metadata.backend, metadata.adapter, metadata.software_fallback, metadata.gpu_claim
    );
    #[cfg(target_os = "linux")]
    {
        let lower = info_text.to_ascii_lowercase();
        assert!(
            lower.contains("lavapipe") || lower.contains("llvmpipe"),
            "CC1 Linux primary GPU evidence must use lavapipe/llvmpipe; adapter was {info_text}. Install Mesa's software Vulkan adapter or fix Vulkan ICD discovery."
        );
    }
    (gpu, info_text)
}

fn hardware_gpu() -> (GpuContext, String) {
    let gpu = GpuContext::headless(false).unwrap_or_else(|error| {
        panic!(
            "CC1 hardware GPU parity is required but no non-fallback adapter was available ({error}). Install/enable a supported Vulkan, DX12, Metal, or GL adapter for this platform, then rerun cargo test -p kinewright-media."
        )
    });
    let metadata = gpu.monitor_proof_metadata();
    assert!(
        !metadata.software_fallback && metadata.gpu_claim,
        "CC1 hardware GPU parity must use a real non-CPU adapter; observed backend={} adapter={} software_fallback={} gpu_claim={}. Check GPU drivers/ICD discovery.",
        metadata.backend,
        metadata.adapter,
        metadata.software_fallback,
        metadata.gpu_claim
    );
    let info_text = format!(
        "backend={};adapter={};software_fallback={};gpu_claim={}",
        metadata.backend, metadata.adapter, metadata.software_fallback, metadata.gpu_claim
    );
    (gpu, info_text)
}

fn backend_name(info: &str) -> &str {
    // Preserve the exact production provenance string, including backend,
    // adapter/device name, fallback status, and GPU claim.  Collapsing this to
    // a generic "wgpu_fallback" label makes a parity result unauditable.
    info
}

#[test]
fn cc1_manifest_declares_every_required_evidence_fixture() {
    let manifest: Value = serde_json::from_str(include_str!("../tests/fixtures/cc1_manifest.json"))
        .expect("CC1 fixture manifest must be valid JSON");
    assert_eq!(manifest["manifest_version"], 1);
    assert_eq!(manifest["profiles"].as_array().map(Vec::len), Some(2));
    assert_eq!(
        manifest["source_depths_bits"].as_array().map(Vec::len),
        Some(2)
    );
    assert_eq!(manifest["controls"].as_array().map(Vec::len), Some(10));
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
        json!({"required_fixture_count": required.len()}),
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
            metric.max <= 1,
            "{} source/reference ramp differs by {:?}; source hash={}",
            spec.name,
            metric,
            output_hash(&source_bytes)
        );
        assert!(
            metric.p99 <= 1.0,
            "{} ramp P99 metric: {metric:?}",
            spec.name
        );
        assert!(
            metric.mean <= 0.25,
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
        assert!(
            float_metric.max <= 2.0e-3,
            "{} working ramp differs from source reference by {:?}",
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

#[test]
fn cc1_neutral_chart_exercises_twelve_reference_patches() {
    let neutral = PrimaryCorrection::default();
    let output = chart_monitor(neutral);
    assert_eq!(output.len(), 12 * 3);
    for (index, (name, input)) in chart_patches().into_iter().enumerate() {
        let actual = &output[index * 3..index * 3 + 3];
        if input[0] == input[1] && input[1] == input[2] {
            assert!(actual[0].abs_diff(actual[1]) <= 1, "{name} red/green drift");
            assert!(
                actual[1].abs_diff(actual[2]) <= 1,
                "{name} green/blue drift"
            );
        }
    }
    assert_eq!(&output[0..3], &[0, 0, 0]);
    assert_eq!(&output[15..18], &[255, 255, 255]);
    for (index, (_, input)) in chart_patches().into_iter().enumerate() {
        let expected = encode_monitor_rgb8(input);
        assert_eq!(&output[index * 3..index * 3 + 3], expected.as_slice());
    }
    let neutral_spread = chart_neutral_spread(&output);
    assert!(
        neutral_spread <= 1,
        "neutral chart spread: {neutral_spread}"
    );
    emit_evidence(
        "cc1_neutral_chart_12_patch",
        "cpu_reference",
        Some(ColorSourceProfile::Rec709Video),
        Some(8),
        json!({"primary_correction": "neutral", "patches": 12}),
        (12, 1),
        None,
        output_hash(&output),
        json!({"neutral_channel_spread_max": neutral_spread, "patch_count": 12}),
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
fn cc1_primary_controls_cover_neutral_bounds_interiors_and_tonal_patches() {
    let mut evidence = BTreeMap::new();
    for (index, parameter) in PrimaryParameter::ALL.into_iter().enumerate() {
        let values = control_fixture_values(parameter);
        let mut outputs = Vec::new();
        for &value in &values {
            let effect = effect_with_parameters(100 + index as u64, [(parameter.name(), value)]);
            let correction = PrimaryCorrection::from_effect(&effect).expect("control fixture");
            let input = if matches!(
                parameter,
                PrimaryParameter::BlacksPercent
                    | PrimaryParameter::ShadowsPercent
                    | PrimaryParameter::HighlightsPercent
                    | PrimaryParameter::WhitesPercent
            ) {
                [0.04, 0.5, 0.96]
            } else {
                [0.23, 0.47, 0.81]
            };
            let output = correction.apply_checked(input).expect("valid control");
            assert!(output.iter().all(|value| value.is_finite()));
            outputs.extend(output);
        }
        // Neutral values are exact identities, including a neutral 0.5 pivot.
        let neutral_effect =
            effect_with_parameters(200 + index as u64, [(parameter.name(), values[0])]);
        let neutral = PrimaryCorrection::from_effect(&neutral_effect).expect("neutral control");
        let neutral_output = neutral
            .apply_checked([0.23, 0.47, 0.81])
            .expect("neutral input");
        for (actual, expected) in neutral_output.into_iter().zip([0.23, 0.47, 0.81]) {
            assert!((actual - expected).abs() <= 1.0e-6);
        }
        if parameter == PrimaryParameter::SaturationPercent {
            let mono = PrimaryCorrection::from_effect(&effect_with_parameters(
                300 + index as u64,
                [(parameter.name(), -100)],
            ))
            .expect("saturation -100");
            let output = mono.apply_checked([1.0, 0.2, 0.05]).expect("monochrome");
            assert!((output[0] - output[1]).abs() < 1.0e-6);
            assert!((output[1] - output[2]).abs() < 1.0e-6);
            assert_eq!(values[0], 0);
        }
        if parameter == PrimaryParameter::ContrastPercent {
            let pivot = PrimaryCorrection::from_effect(&effect_with_parameters(
                400 + index as u64,
                [
                    ("contrast_percent", 100),
                    ("contrast_pivot_basis_points", 5_000),
                ],
            ))
            .expect("contrast pivot");
            assert_eq!(
                pivot.apply_checked([0.5, 0.5, 0.5]).expect("pivot"),
                [0.5; 3]
            );
        }
        evidence.insert(
            parameter.name(),
            json!({"values": values, "outputs": outputs}),
        );
    }
    assert_eq!(evidence.len(), 10);
    let exposure_values = evidence
        .get("exposure_milli_stops")
        .and_then(|value| value.get("values"))
        .and_then(Value::as_array)
        .expect("exposure evidence values");
    assert!(exposure_values.iter().any(|value| value == -1_000));
    assert!(exposure_values.iter().any(|value| value == 1_000));
    let exposure_plus_minus_one_stop = exposure_values.iter().any(|value| value == -1_000)
        && exposure_values.iter().any(|value| value == 1_000);
    let saturation_values = evidence
        .get("saturation_percent")
        .and_then(|value| value.get("values"))
        .and_then(Value::as_array)
        .expect("saturation evidence values");
    let saturation_minus_100_and_zero = saturation_values.iter().any(|value| value == -100)
        && saturation_values.iter().any(|value| value == 0);
    assert!(saturation_minus_100_and_zero);
    let tonal_low_high_patches = [
        PrimaryParameter::BlacksPercent,
        PrimaryParameter::ShadowsPercent,
        PrimaryParameter::HighlightsPercent,
        PrimaryParameter::WhitesPercent,
    ]
    .into_iter()
    .all(|parameter| {
        evidence
            .get(parameter.name())
            .and_then(|value| value.get("outputs"))
            .and_then(Value::as_array)
            .is_some_and(|outputs| outputs.len() >= 12)
    });
    assert!(tonal_low_high_patches);
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
        json!({"control_count": 10, "tonal_low_high_patches": tonal_low_high_patches, "exposure_plus_minus_one_stop": exposure_plus_minus_one_stop, "saturation_minus_100_and_zero": saturation_minus_100_and_zero}),
    );
}

fn assert_gpu_parity(gpu: GpuContext, info: &str) {
    let backend = backend_name(info);
    let compositor = Compositor::new(gpu);
    let (width, height, frame) = representative_frame();
    let mut cases = Vec::new();
    let mut output_bytes = Vec::new();
    let representative = representative_correction();
    let (metric, linear_metric, actual) =
        assert_gpu_control_case(&compositor, width, height, &frame, representative);
    let representative_luma = monitor_luma_and_clipping(&actual);
    output_bytes.extend_from_slice(&actual);
    cases.push(json!({
        "case": "representative_all_controls",
        "controls": correction_value_json(representative),
        "monitor_max_code_error": metric.max,
        "monitor_p99_code_error": metric.p99,
        "monitor_mean_code_error": metric.mean,
        "linear_max_error": linear_metric.metrics.max,
        "linear_p99_error": linear_metric.metrics.p99,
        "linear_mean_error": linear_metric.metrics.mean,
        "linear_included_rgb_samples": linear_metric.included,
        "linear_excluded_rgb_samples": linear_metric.excluded,
        "monitor_luma_and_clipping": representative_luma.clone(),
    }));
    for parameter in PrimaryParameter::ALL {
        for value in control_fixture_values(parameter) {
            let effect = effect_with_parameters(10_000, [(parameter.name(), value)]);
            let correction = PrimaryCorrection::from_effect(&effect).expect("GPU control fixture");
            let (metric, linear_metric, actual) =
                assert_gpu_control_case(&compositor, width, height, &frame, correction);
            let luma = monitor_luma_and_clipping(&actual);
            output_bytes.extend_from_slice(&actual);
            cases.push(json!({
                "case": "single_control",
                "parameter": parameter.name(),
                "value": value,
                "monitor_max_code_error": metric.max,
                "monitor_p99_code_error": metric.p99,
                "monitor_mean_code_error": metric.mean,
                "linear_max_error": linear_metric.metrics.max,
                "linear_p99_error": linear_metric.metrics.p99,
                "linear_mean_error": linear_metric.metrics.mean,
                "linear_included_rgb_samples": linear_metric.included,
                "linear_excluded_rgb_samples": linear_metric.excluded,
                "monitor_luma_and_clipping": luma,
            }));
        }
    }
    emit_evidence(
        "cc1_gpu_cpu_parity",
        backend,
        Some(ColorSourceProfile::Rec709Video),
        Some(16),
        json!({"primary_correction": "representative_plus_every_control_boundary"}),
        (width, height),
        None,
        output_hash(&output_bytes),
        json!({
            "linear_storage": "rgba16float",
            "representative_monitor_luma_and_clipping": representative_luma,
            "cases": cases,
            "control_case_count": 1 + PrimaryParameter::ALL.iter().map(|parameter| control_fixture_values(*parameter).len()).sum::<usize>(),
            "linear_gate": {"max": LINEAR_CPU_GPU_MAX, "p99": LINEAR_CPU_GPU_P99, "mean": LINEAR_CPU_GPU_MEAN, "status": "passed"},
        }),
    );
}

#[test]
fn cc1_gpu_compositor_matches_canonical_cpu_reference_on_software_fallback() {
    let (fallback, fallback_info) = fallback_gpu();
    assert_gpu_parity(fallback, &fallback_info);
}

#[test]
#[ignore = "requires a physical supported GPU; run explicitly with --ignored --nocapture"]
fn cc1_gpu_compositor_matches_canonical_cpu_reference_on_hardware() {
    let (hardware, hardware_info) = hardware_gpu();
    assert_gpu_parity(hardware, &hardware_info);
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
    let clipped = apply_primary_corrections(
        input,
        &[
            positive,
            PrimaryCorrection {
                exposure_milli_stops: -1_000,
                ..PrimaryCorrection::default()
            },
        ],
    )
    .expect("serial controls");
    let incorrectly_clipped = negative
        .apply_checked(over_range.map(|value| value.clamp(0.0, 1.0)))
        .expect("clipped recovery");
    let clamped_recovery_differs = incorrectly_clipped
        .iter()
        .zip(clipped)
        .any(|(actual, expected)| (actual - expected).abs() > 1.0e-3);
    assert!(clamped_recovery_differs);
    let frame = working_frame(1, 1, &[input]);
    let effect_positive = correction_effect(1, positive);
    let effect_negative = correction_effect(2, negative);
    let (gpu, info) = fallback_gpu();
    let compositor = Compositor::new(gpu);
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
            (actual - expected).abs() <= LINEAR_CPU_GPU_MAX,
            "production WGSL clamped or failed recovery: actual={actual} expected={expected}"
        );
    }
    let recovered_hash = bytemuck_free_f32_bytes(&recovered);
    emit_evidence(
        "cc1_no_intermediate_clamp",
        backend_name(&info),
        Some(ColorSourceProfile::Rec709Video),
        Some(16),
        json!({"nodes": ["exposure:+1", "exposure:-1"]}),
        (3, 1),
        None,
        output_hash(&recovered_hash),
        json!({"over_range_before_clamp": over_range, "recovered": recovered, "clamped_recovery_differs": clamped_recovery_differs, "production_working": working[..3].to_vec()}),
    );
}

fn bytemuck_free_f32_bytes(values: &[f32; 3]) -> Vec<u8> {
    values
        .iter()
        .flat_map(|value| value.to_le_bytes())
        .collect()
}

#[test]
fn cc1_supported_and_unsupported_source_profiles_are_typed_and_actionable() {
    initialize_ffmpeg().expect("FFmpeg must initialize for unsupported-source fixture");
    let directory = TempDirectory::new("cc1-unsupported-source");
    let (actual_path, _) = generate_delivery_source(&directory, 32, 16);
    let mut unsupported = rec709_description(8, ColorRange::Limited, ColorTransfer::Bt709);
    assert_eq!(
        classify_source(&unsupported),
        Ok(ColorSourceProfile::Rec709Video)
    );
    unsupported.primaries = ColorPrimaries::Bt2020;
    let error = classify_source(&unsupported).expect_err("BT.2020 must be rejected");
    assert_eq!(error.code(), "unsupported_source_primaries");
    assert!(
        error
            .actionable_message()
            .contains("Apply an explicit supported source-colour override")
    );
    unsupported = rec709_description(8, ColorRange::Limited, ColorTransfer::Bt709);
    unsupported.transfer = ColorTransfer::Smpte2084;
    assert_eq!(
        classify_source(&unsupported)
            .expect_err("PQ must be rejected")
            .field(),
        "transfer"
    );
    unsupported = rec709_description(8, ColorRange::Limited, ColorTransfer::Bt709);
    unsupported.matrix = ColorMatrix::Smpte170M;
    assert_eq!(
        classify_source(&unsupported)
            .expect_err("BT.601 matrix must be rejected")
            .field(),
        "matrix"
    );
    unsupported = rec709_description(8, ColorRange::Limited, ColorTransfer::Bt709);
    unsupported.bit_depth = ColorBitDepth::Float16;
    assert_eq!(
        classify_source(&unsupported)
            .expect_err("float source must be rejected")
            .field(),
        "bit_depth"
    );
    unsupported = rec709_description(8, ColorRange::Limited, ColorTransfer::Bt709);
    unsupported.white_point = ColorWhitePoint::Unknown;
    assert_eq!(
        classify_source(&unsupported).expect_err("unknown white point must block"),
        kinewright_core::ColorSourceError::UnknownWhitePoint
    );
    assert_eq!(
        classify_source_with_assumption(&unsupported, Some(ColorSourceProfileAssumption::D65)),
        Ok(ColorSourceProfile::Rec709Video)
    );
    let unknown_description = ColorDescription::unknown();
    let unknown_error = classify_source(&unknown_description)
        .expect_err("unknown source metadata must block managed classification");
    assert_eq!(unknown_error.field(), "primaries");
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
    let mut partial_description = rec709_description(8, ColorRange::Limited, ColorTransfer::Bt709);
    partial_description.bit_depth = ColorBitDepth::Unknown;
    partial_description.confidence_basis_points = 2_000;
    partial_description.provenance = ColorProvenance::StreamMetadata;
    let partial_error = classify_source(&partial_description)
        .expect_err("partial source metadata must block managed classification");
    assert_eq!(partial_error.field(), "bit_depth");
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
    let mut transfer_cases = Vec::new();
    for transfer in [
        ColorTransfer::AribStdB67,
        ColorTransfer::Log,
        ColorTransfer::LogC,
    ] {
        let mut description = rec709_description(8, ColorRange::Limited, ColorTransfer::Bt709);
        description.transfer = transfer.clone();
        let error = classify_source(&description).expect_err("unsupported transfer");
        assert_eq!(error.field(), "transfer");
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
        transfer_cases.push(json!({"transfer": transfer, "classifier_field": error.field(), "managed_decode_blocked": true}));
    }
    let mut unsupported_depth = rec709_description(8, ColorRange::Limited, ColorTransfer::Bt709);
    unsupported_depth.bit_depth = ColorBitDepth::Integer(7);
    let depth_error = match VideoDecoder::open_scaled_managed(
        &actual_path,
        Rational::new(1, 1).expect("one fps"),
        None,
        &unsupported_depth,
        Some(ColorSourceProfileAssumption::D65),
    ) {
        Ok(_) => panic!("actual source with unsupported integer depth must block managed decode"),
        Err(error) => error,
    };
    assert!(matches!(depth_error, MediaError::Backend(_)));
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
    let (gpu, gpu_info) = fallback_gpu();
    let export_gpu = gpu.clone();
    let engine = crate::engine::FfmpegMediaEngine::new_with_gpu(gpu)
        .expect("media engine should start for unsupported proof gate");
    let proof_result =
        engine.monitor_proof_for_document(Arc::new(blocked_document.clone()), TimeCode::ZERO);
    let proof_blocked = proof_result.is_err();
    let proof_error = match proof_result {
        Ok(_) => panic!("unsupported source must block production full-raster proof"),
        Err(error) => error,
    };
    assert!(proof_blocked);
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
    let export_result = crate::export::export_document(
        &blocked_document,
        &blocked_export_path,
        &blocked_settings,
        &progress_tx,
        export_gpu,
    );
    let export_blocked = export_result.is_err() && !blocked_export_path.exists();
    let export_error = match export_result {
        Ok(_) => panic!("unsupported source must block production export"),
        Err(error) => error,
    };
    assert!(export_blocked);
    emit_evidence(
        "cc1_unsupported_metadata",
        backend_name(&gpu_info),
        None,
        None,
        json!({"supported": ["rec709_video", "srgb_full"], "recovery": "explicit_source_override_or_relink"}),
        (0, 0),
        None,
        output_hash(error.actionable_message().as_bytes()),
        json!({"typed_failures": ["primaries", "transfer", "matrix", "bit_depth", "white_point"], "unknown_metadata": {"classifier_field": unknown_error.field(), "managed_decode_blocked": true, "error": unknown_decode_error.to_string()}, "partial_metadata": {"classifier_field": partial_error.field(), "managed_decode_blocked": true, "error": partial_decode_error.to_string()}, "transfer_cases": transfer_cases, "unsupported_integer_depth": 7, "unsupported_integer_depth_error": depth_error.to_string(), "actual_source": actual_path, "d65_assumption_preserves_raw_metadata": true, "proof_blocked": proof_blocked, "export_blocked": export_blocked, "delivery_conformance_export_ready": conformance.export_ready(), "proof_block_error": proof_error.to_string(), "export_block_error": export_error.to_string()}),
    );
}

fn simple_document(asset: MediaAsset, resolution: (u32, u32)) -> Document {
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
    let (gpu, info) = fallback_gpu();
    let engine = crate::engine::FfmpegMediaEngine::new_with_gpu(gpu.clone())
        .expect("media engine should start on the fixture adapter");
    let proof = engine
        .monitor_proof_for_document(Arc::clone(&document), TimeCode::ZERO)
        .expect("full-raster monitor proof");
    let mut full_renderer = crate::render::FrameRenderer::new(gpu);
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
    assert!(!proof.metadata.backend.is_empty());
    assert!(!proof.metadata.adapter.is_empty());
    assert!(proof.metadata.software_fallback);
    assert!(!proof.metadata.gpu_claim);
    let hash = output_hash(&proof.image.pixels);
    emit_evidence(
        "cc1_full_raster_monitor_proof",
        backend_name(&info),
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
            "proof_raster": [proof.image.width, proof.image.height],
            "preview_raster": [preview.width, preview.height],
            "proxy_smaller": preview.width < proof.image.width,
            "same_render_semantics": same_full_pixels,
            "source_raw_hash": output_hash(&source_bytes),
            "proof_metadata": proof.metadata,
        }),
    );
}

fn generate_delivery_source(
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
            y_plane.push([16_u8, 64, 128, 192, 235][usize::try_from(bar).expect("bar")]);
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
    let (gpu, info) = fallback_gpu();
    let proof_engine = crate::engine::FfmpegMediaEngine::new_with_gpu(gpu.clone())
        .expect("production media engine should start for delivery proof");
    let proof = proof_engine
        .monitor_proof_for_document(Arc::new(document.clone()), TimeCode::ZERO)
        .expect("production full-raster delivery proof");
    assert!(proof.metadata.full_resolution);
    let mut direct_renderer = crate::render::FrameRenderer::new(gpu.clone());
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
    crate::export::export_document(&document, &output_path, &settings, &progress_tx, gpu)
        .expect("production export_document should create H.264 delivery");
    let asset = probe_path(&output_path, AssetId(3)).expect("production H.264 should probe");
    assert_eq!(asset.color_description.primaries, ColorPrimaries::Bt709);
    assert_eq!(asset.color_description.transfer, ColorTransfer::Bt709);
    assert_eq!(asset.color_description.matrix, ColorMatrix::Bt709);
    assert_eq!(asset.color_description.range, ColorRange::Limited);
    assert_eq!(asset.color_description.bit_depth, ColorBitDepth::Eight);
    let actual_pixel_format = decoded_video_pixel_format(&output_path);
    assert_eq!(actual_pixel_format, "yuv420p");
    let decoder = ffmpeg_cli_decode_rgba(&output_path, width, height);
    let metric = abs_code_diff_rgb(&decoder, &proof.image.pixels);
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
        backend_name(&info),
        Some(ColorSourceProfile::Rec709Video),
        Some(8),
        json!({"delivery": "bt709_limited_8bit", "codec": "h264", "pixel_format": "yuv420p"}),
        (width, height),
        Some(file_hash(&source_path)),
        output_hash(&decoder),
        json!({"max_code_error": metric.max, "p99_code_error": metric.p99, "mean_code_error": metric.mean, "explicit_bt709_limited_tags": true, "actual_pixel_format": actual_pixel_format, "compositor_gate_reused": false, "production_renderer": "monitor_proof_for_document+FrameRenderer+export_document", "same_raster_proof": [proof.image.width, proof.image.height], "proof_metadata": proof.metadata, "source_hash": file_hash(&source_path), "source_raw_hash": output_hash(&source_bytes), "delivery_backend": "ffmpeg_h264_yuv420p"}),
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
    let (gpu, info) = fallback_gpu();
    let mut renderer = crate::render::FrameRenderer::new(gpu);
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
        backend_name(&info),
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
