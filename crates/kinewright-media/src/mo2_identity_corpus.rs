//! MO2 R15 / CC8 G2: the `Normal`-only identity corpus and its byte record.
//!
//! This file compiles unchanged against the pre-MO2 tree (97fc937) so the
//! pre-MO2 baselines under `tests/fixtures/mo2_normal_identity/` were
//! recorded by this exact code there; the pins in `mo2_fixtures` compare the
//! post-MO2 renders to them byte-for-byte under the same adapter. To
//! re-record after a driver change, run it on the pre-MO2 commit — never
//! on a post-MO2 tree, whose output is what is under test.

use std::path::{Path, PathBuf};

use kinewright_core::{
    AssetId, ClipContent, ColorBitDepth, ColorDescription, ColorMatrix, ColorPrimaries,
    ColorProvenance, ColorRange, ColorTransfer, ColorWhitePoint, Document, Effect, EffectId,
    ParamValue, TimeCode, Title, Track, TrackId, TrackKind, Transition,
};

use crate::compositor::GpuContext;
use crate::decode::probe_path;
use crate::render::{DecodeStrategy, FrameRenderer, RenderScale};
use crate::test_support::{GeneratedMedia, single_clip_document};

/// One rendered corpus frame: label, linear working (f16 LE, the
/// accumulator's storage precision, so still exact), monitor RGBA8 and
/// delivery RGBA64LE bytes.
pub(crate) struct Record {
    pub(crate) label: String,
    pub(crate) working: Vec<u8>,
    pub(crate) monitor: Vec<u8>,
    pub(crate) delivery: Vec<u8>,
}

pub(crate) fn source() -> GeneratedMedia {
    GeneratedMedia::ffmpeg(
        "mo2-normal-identity",
        &[
            "-f",
            "lavfi",
            "-i",
            "testsrc2=size=64x36:rate=25:duration=1",
            "-frames:v",
            "10",
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

fn effect(id: u64, name: &str, parameters: &[(&str, i64)]) -> Effect {
    Effect {
        id: EffectId(id),
        name: name.to_owned(),
        parameters: parameters
            .iter()
            .map(|(key, value)| ((*key).to_owned(), ParamValue::Integer(*value)))
            .collect(),
        keyframes: std::collections::BTreeMap::new(),
        enabled: true,
        enabled_curve: None,
    }
}

/// The corpus: `(label, document, project frame)`, every layer `Normal`,
/// every feature pre-MO2 (media, transform, opacity, crop, mask, colour
/// nodes, the legacy stage, titles, crossfade, colour fade, over-range).
#[allow(clippy::too_many_lines)]
pub(crate) fn corpus(media: &Path) -> Vec<(&'static str, Document, i64)> {
    let mut asset = probe_path(media, AssetId(1)).expect("the corpus source probes");
    asset.color_description = ColorDescription {
        primaries: ColorPrimaries::Bt709,
        transfer: ColorTransfer::Bt709,
        matrix: ColorMatrix::Bt709,
        range: ColorRange::Limited,
        white_point: ColorWhitePoint::D65,
        bit_depth: ColorBitDepth::Eight,
        confidence_basis_points: 10_000,
        provenance: ColorProvenance::UserOverride,
    };
    let base = single_clip_document(asset);
    let with = |effects: Vec<Effect>| {
        let mut document = base.clone();
        document.tracks[0].clips[0].effects = effects;
        document
    };
    let above = |content: ClipContent, transition: Option<&str>| {
        let mut document = base.clone();
        let mut clip = document.tracks[0].clips[0].clone();
        clip.id = kinewright_core::ClipId(2);
        clip.content = content;
        clip.effects = vec![effect(9, "opacity", &[("percent", 85)])];
        clip.transition_in = transition.map(|name| Transition {
            name: name.to_owned(),
            duration: TimeCode(5),
        });
        document.tracks.push(Track {
            id: TrackId(2),
            kind: TrackKind::Video,
            sync_lock: true,
            clips: vec![clip],
        });
        document
    };
    let title = || {
        ClipContent::Title(Title {
            text: "MO2".to_owned(),
            ..Title::default()
        })
    };
    vec![
        ("plain", with(Vec::new()), 3),
        (
            "transform+opacity",
            with(vec![
                effect(
                    1,
                    "transform",
                    &[
                        ("scale_percent", 80),
                        ("rotation_centidegrees", 700),
                        ("x_percent", 5),
                    ],
                ),
                effect(2, "opacity", &[("percent", 90)]),
            ]),
            3,
        ),
        (
            "crop+mask+grade",
            with(vec![
                effect(1, "crop", &[("left_percent", 10), ("top_percent", 5)]),
                effect(
                    2,
                    "mask",
                    &[
                        ("shape_token", 2),
                        ("width_percent", 70),
                        ("feather_percent", 30),
                    ],
                ),
                effect(
                    3,
                    "primary_correction",
                    &[("exposure_milli_stops", 300), ("saturation_percent", -20)],
                ),
            ]),
            3,
        ),
        (
            "legacy stage",
            with(vec![
                effect(1, "saturation", &[("percent", -40)]),
                effect(2, "brightness", &[("percent", 10)]),
            ]),
            3,
        ),
        (
            "over-range Normal",
            with(vec![effect(
                1,
                "primary_correction",
                &[("exposure_milli_stops", 3_000)],
            )]),
            3,
        ),
        ("title over media", above(title(), None), 3),
        ("crossfade", above(ClipContent::Media, Some("crossfade")), 2),
        (
            "fade_from_black title",
            above(title(), Some("fade_from_black")),
            2,
        ),
    ]
}

/// Render every corpus frame through the working, monitor and delivery
/// entries.
pub(crate) fn render(renderer: &mut FrameRenderer, media: &Path) -> Vec<Record> {
    let (full, seek) = (RenderScale::FullResolution, DecodeStrategy::Seek);
    corpus(media)
        .into_iter()
        .map(|(label, document, at)| {
            let (at, size) = (TimeCode(at), document.resolution);
            let working = renderer.render_working(&document, at, size, full, seek);
            let monitor = renderer.render(&document, at, size, full, seek);
            let delivery = renderer.render_delivery(&document, at, size, full, seek);
            let working = working.unwrap_or_else(|error| panic!("{label} working: {error}"));
            Record {
                label: label.to_owned(),
                working: working
                    .pixels
                    .iter()
                    .flat_map(|v| half::f16::from_f32(*v).to_le_bytes())
                    .collect(),
                monitor: monitor.expect("the corpus renders").rgba.to_vec(),
                delivery: delivery.expect("the corpus delivers").rgba64le,
            }
        })
        .collect()
}

/// The baseline file for this adapter: one per environment (R15, CC8 G2).
pub(crate) fn baseline_path(gpu: &GpuContext) -> PathBuf {
    let metadata = gpu.monitor_proof_metadata();
    let key: String = format!("{}-{}", metadata.backend, metadata.adapter)
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .collect();
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/mo2_normal_identity")
        .join(format!("{key}.bin"))
}

#[allow(dead_code, reason = "the pre-MO2 recorder writes baselines with it")]
fn put(out: &mut Vec<u8>, bytes: &[u8]) {
    out.extend_from_slice(&u32::try_from(bytes.len()).unwrap().to_le_bytes());
    out.extend_from_slice(bytes);
}

#[allow(dead_code, reason = "the pre-MO2 recorder writes baselines with it")]
pub(crate) fn encode(records: &[Record]) -> Vec<u8> {
    let mut out = Vec::new();
    for record in records {
        put(&mut out, record.label.as_bytes());
        put(&mut out, &record.working);
        put(&mut out, &record.monitor);
        put(&mut out, &record.delivery);
    }
    out
}

fn take(bytes: &mut &[u8]) -> Vec<u8> {
    let (length, rest) = bytes.split_at(4);
    let length = u32::from_le_bytes(length.try_into().unwrap()) as usize;
    let (field, rest) = rest.split_at(length);
    *bytes = rest;
    field.to_vec()
}

pub(crate) fn decode(mut bytes: &[u8]) -> Vec<Record> {
    let mut records = Vec::new();
    while !bytes.is_empty() {
        records.push(Record {
            label: String::from_utf8(take(&mut bytes)).unwrap(),
            working: take(&mut bytes),
            monitor: take(&mut bytes),
            delivery: take(&mut bytes),
        });
    }
    records
}
