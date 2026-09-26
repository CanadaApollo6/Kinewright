//! MO2 R24/R25: `preview_solo`, one clip's contribution as a sampled strip.
//!
//! Render-only over an immutable snapshot: a derived copy hides the other
//! video clips (disable, never delete, so track indices hold) and every
//! sample goes through the monitor-proof path. Budgets degrade samples, then
//! thumbnail bounds, and otherwise fail closed JSON-only.

use std::{sync::Arc, time::Instant};

use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use image::{ColorType, ImageEncoder as _, codecs::png::PngEncoder, imageops};
use kinewright_core::{
    Analysis, ClipContent, ClipId, Document, RgbaImage, TimeCode, TimelineRevision, TrackKind,
};
use rmcp::model::{CallToolResult, ContentBlock};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use thiserror::Error;

pub const SOLO_REPORT_BUDGET_BYTES: usize = 4 * 1024;
pub const SOLO_PNG_BUDGET_BYTES: usize = 768 * 1024;
pub const SOLO_WIRE_BUDGET_BYTES: usize = 1_056 * 1024;
const MAX_THUMB_PIXELS: usize = 3_276_800;
const MAX_FULL_SIDE: usize = 8_192;
const MAX_FULL_PIXELS: usize = 16_777_216;

/// MO2 R24: what the soloed layer is drawn over.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SoloContext {
    /// The layer alone over black.
    Isolated,
    /// The layer over its true below-stack, above-layers hidden.
    Below,
}

/// MO2 R24: `preview_solo` arguments.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SoloArgs {
    /// The timeline revision the strip is rendered against.
    pub expected_revision: TimelineRevision,
    /// The video-track clip to solo.
    pub clip_id: ClipId,
    /// Sample count, 2..=16 (default 8); ignored when `full_res` is true.
    #[serde(default = "default_samples")]
    pub samples: u32,
    /// Default by blend: `normal` isolated, other blends below. Adjustments are always below.
    #[serde(default)]
    pub context: Option<SoloContext>,
    /// One full-resolution midpoint (paired for adjustments) instead of thumbnails.
    #[serde(default)]
    pub full_res: bool,
}

const fn default_samples() -> u32 {
    8
}

/// MO2 R24: why a solo strip was refused. Typed, never an incident (R29).
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum SoloError {
    #[error("clip {clip} is not visible: {reason}")]
    SoloClipNotVisible { clip: ClipId, reason: &'static str },
    #[error("clip {clip} spans no project frames")]
    SoloWindowEmpty { clip: ClipId },
    #[error("solo response over budget: {limit} is {observed}, allowed {allowed}")]
    SoloOverBudget {
        limit: &'static str,
        observed: usize,
        allowed: usize,
    },
    #[error("samples must be 2..=16, got {0}")]
    InvalidSamples(u32),
    #[error("solo render failed: {0}")]
    RenderFailed(String),
}

impl SoloError {
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::SoloClipNotVisible { .. } => "solo_clip_not_visible",
            Self::SoloWindowEmpty { .. } => "solo_window_empty",
            Self::SoloOverBudget { .. } => "solo_over_budget",
            Self::InvalidSamples(_) => "solo_invalid_samples",
            Self::RenderFailed(_) => "solo_render_failed",
        }
    }

    /// The JSON-only error body every seam carries.
    #[must_use]
    pub fn body(&self) -> Value {
        json!({"code": self.code(), "message": self.to_string(), "applied": false})
    }
}

/// One rendered strip: the composed image, its PNG and the sampling report.
#[derive(Debug, Clone)]
pub struct SoloStrip {
    pub image: RgbaImage,
    pub png: Vec<u8>,
    pub report: Value,
}

impl SoloStrip {
    /// The MCP response: summary text, the strip PNG and the report.
    #[must_use]
    pub fn to_result(&self) -> CallToolResult {
        let report = &self.report;
        let text = format!(
            "preview_solo clip={} context={} emitted={} of {} strip={}x{}",
            report["clip_id"],
            report["context"].as_str().unwrap_or_default(),
            report["emitted"],
            report["requested"],
            self.image.width,
            self.image.height
        );
        let mut result = CallToolResult::success(vec![
            ContentBlock::text(text),
            ContentBlock::image(BASE64.encode(&self.png), "image/png"),
        ]);
        result.structured_content = Some(report.clone());
        result
    }
}

/// Render `args.clip_id` solo over `document` (already revision-checked).
///
/// # Errors
///
/// A typed [`SoloError`]; nothing is applied either way.
#[allow(clippy::too_many_lines)]
pub fn preview_solo(
    analysis: &dyn Analysis,
    revision: TimelineRevision,
    document: &Document,
    args: &SoloArgs,
) -> Result<SoloStrip, SoloError> {
    let started = Instant::now();
    let clip_id = args.clip_id;
    let not_visible = |reason| SoloError::SoloClipNotVisible {
        clip: clip_id,
        reason,
    };
    let (track, kind, clip) = (document.tracks.iter().enumerate())
        .find_map(|(index, track)| {
            let clip = track.clips.iter().find(|clip| clip.id == clip_id)?;
            Some((index, track.kind, clip))
        })
        .ok_or_else(|| not_visible("no such clip"))?;
    if kind != TrackKind::Video {
        return Err(not_visible("the clip is on an audio track"));
    }
    if !args.full_res && !(2..=16).contains(&args.samples) {
        return Err(SoloError::InvalidSamples(args.samples));
    }
    let failed = |error: &dyn std::fmt::Display| SoloError::RenderFailed(error.to_string());
    let length = document.clip_duration(clip).map_err(|e| failed(&e))?.0;
    let Ok(span) = usize::try_from(length) else {
        return Err(SoloError::SoloWindowEmpty { clip: clip_id });
    };
    if span == 0 {
        return Err(SoloError::SoloWindowEmpty { clip: clip_id });
    }
    let adjustment = matches!(clip.content, ClipContent::Adjustment);
    let context = match (adjustment, args.context) {
        (false, Some(context)) => context,
        (false, None) if clip.blend_mode.is_normal() => SoloContext::Isolated,
        _ => SoloContext::Below,
    };
    let rows = if adjustment { 2 } else { 1 };
    let (width, height) = (
        document.resolution.0 as usize,
        document.resolution.1 as usize,
    );
    let full_pixels = width * height * rows;
    if args.full_res && width.max(height) > MAX_FULL_SIDE {
        return Err(over("full_res_side", width.max(height), MAX_FULL_SIDE));
    }
    if args.full_res && full_pixels > MAX_FULL_PIXELS {
        return Err(over(
            "full_res_decoded_pixels",
            full_pixels,
            MAX_FULL_PIXELS,
        ));
    }
    let requested = if args.full_res {
        1
    } else {
        args.samples as usize
    };
    let emitted = requested.min(span).min(16 / rows);
    let offsets = if args.full_res {
        vec![(length - 1) / 2]
    } else {
        sample_offsets(length, emitted)
    };

    // Hide by disabling, never deleting, so track indices (and the stack) hold.
    let hide = |keep: &dyn Fn(usize, ClipId) -> bool| {
        let mut derived = document.clone();
        for (index, track) in derived.tracks.iter_mut().enumerate() {
            let video = track.kind == TrackKind::Video;
            for other in track
                .clips
                .iter_mut()
                .filter(|c| video && !keep(index, c.id))
            {
                (other.enabled, other.enabled_curve) = (false, None);
            }
        }
        Arc::new(derived)
    };
    let below = context == SoloContext::Below;
    let after = hide(&|index, id| id == clip_id || (below && index < track));
    let before = adjustment.then(|| hide(&|index, _| index < track));
    let mut provenance = None;
    let mut samples = Vec::with_capacity(offsets.len());
    for offset in offsets {
        let at = TimeCode(clip.timeline_start.0 + offset);
        let mut proofs = Vec::with_capacity(rows);
        if clip.is_enabled_at(TimeCode(offset)) {
            for derived in before.iter().chain([&after]) {
                let proof = (analysis.monitor_proof_for_document(Arc::clone(derived), at))
                    .map_err(|e| failed(&e))?;
                provenance.get_or_insert(proof.metadata);
                proofs.push(proof.image);
            }
        }
        samples.push((offset, proofs));
    }

    let fallback = emitted.min(2);
    let bounds_label = if fallback < emitted {
        "samples_then_bounds"
    } else {
        "bounds"
    };
    let mut attempts = if args.full_res {
        vec![(1, None, None)]
    } else {
        vec![
            (emitted, Some(320), None),
            (fallback, Some(320), Some("samples")),
            (fallback, Some(160), Some(bounds_label)),
        ]
    };
    attempts.dedup_by_key(|(count, bound, _)| (*count, *bound));
    let mut failure = over("thumbnail_decoded_pixels", 0, 0);
    'attempts: for (count, bound, degraded) in attempts {
        let keep = sample_offsets(length, count);
        let chosen: Vec<_> = if args.full_res {
            samples.iter().collect()
        } else {
            samples
                .iter()
                .filter(|(offset, _)| keep.contains(offset))
                .collect()
        };
        let (cell_w, cell_h) = bound.map_or((width, height), |b| fit(width, height, b));
        let decoded = chosen.len() * rows * cell_w * cell_h;
        if bound.is_some() && decoded > MAX_THUMB_PIXELS {
            failure = over("thumbnail_decoded_pixels", decoded, MAX_THUMB_PIXELS);
            continue;
        }
        let image = compose(&chosen, rows, cell_w, cell_h);
        let mut png = Vec::new();
        PngEncoder::new(&mut png)
            .write_image(
                &image.pixels,
                image.width,
                image.height,
                ColorType::Rgba8.into(),
            )
            .map_err(|e| failed(&e))?;
        let report = json!({
            "clip_id": clip_id,
            "revision": revision,
            "context": context,
            "pairs": adjustment,
            "layout": if adjustment { "before_row_over_after_row" } else { "row" },
            "full_res": args.full_res,
            "requested": requested,
            "emitted": chosen.len(),
            "span_frames": length,
            "degraded": degraded,
            "cell": {"width": cell_w, "height": cell_h},
            "strip": {"width": image.width, "height": image.height},
            "provenance": provenance,
            "png_bytes": png.len(),
            "elapsed_ms": started.elapsed().as_millis(),
            "samples": chosen.iter().map(|(offset, proofs)| {
                let frame = clip.timeline_start.0 + offset;
                if proofs.is_empty() {
                    json!({"frame": frame, "active": false, "reason": "clip_disabled"})
                } else {
                    let hashes: Vec<_> = proofs.iter().map(|p| fnv64(&p.pixels)).collect();
                    json!({"frame": frame, "active": true, "hashes": hashes})
                }
            }).collect::<Vec<_>>(),
        });
        let solo = SoloStrip { image, png, report };
        let wire = serde_json::to_vec(&solo.to_result()).map_or(usize::MAX, |wire| wire.len());
        for (limit, observed, allowed) in [
            ("png_bytes", solo.png.len(), SOLO_PNG_BUDGET_BYTES),
            (
                "report_bytes",
                solo.report.to_string().len(),
                SOLO_REPORT_BUDGET_BYTES,
            ),
            ("response_bytes", wire, SOLO_WIRE_BUDGET_BYTES),
        ] {
            if observed > allowed {
                failure = over(limit, observed, allowed);
                continue 'attempts;
            }
        }
        return Ok(solo);
    }
    Err(failure)
}

const fn over(limit: &'static str, observed: usize, allowed: usize) -> SoloError {
    SoloError::SoloOverBudget {
        limit,
        observed,
        allowed,
    }
}

/// R24: `floor(i×(L−1)/(k−1))`, or `[0]` for `k = 1`.
#[must_use]
pub fn sample_offsets(length: i64, count: usize) -> Vec<i64> {
    let count = i64::try_from(count).unwrap_or(i64::MAX);
    if count <= 1 {
        return vec![0];
    }
    (0..count).map(|i| i * (length - 1) / (count - 1)).collect()
}

/// Aspect-preserving fit inside `bound × 2·bound`, never upscaling.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss
)]
fn fit(width: usize, height: usize, bound: usize) -> (usize, usize) {
    let scale = (bound as f64 / width as f64)
        .min(2.0 * bound as f64 / height as f64)
        .min(1.0);
    let scaled = |side: usize| ((side as f64 * scale).round() as usize).max(1);
    (scaled(width), scaled(height))
}

/// Samples left to right; adjustment pairs put BEFORE over AFTER; inactive
/// cells are flat grey.
#[allow(clippy::cast_possible_truncation, clippy::cast_possible_wrap)] // ≤ 16 cells of ≤ 8192
fn compose(samples: &[&(i64, Vec<RgbaImage>)], rows: usize, w: usize, h: usize) -> RgbaImage {
    let (cell_w, cell_h) = (w as u32, h as u32);
    let (width, height) = (samples.len() as u32 * cell_w, rows as u32 * cell_h);
    let mut canvas = image::RgbaImage::from_pixel(width, height, image::Rgba([0, 0, 0, 255]));
    let grey = image::RgbaImage::from_pixel(cell_w, cell_h, image::Rgba([48, 48, 48, 255]));
    for (column, (_, proofs)) in samples.iter().enumerate() {
        for row in 0..rows {
            let full = proofs.get(row).and_then(|proof| {
                image::RgbaImage::from_raw(proof.width, proof.height, proof.pixels.clone())
            });
            let cell = match full {
                Some(full) if full.dimensions() != (cell_w, cell_h) => {
                    imageops::thumbnail(&full, cell_w, cell_h)
                }
                other => other.unwrap_or_else(|| grey.clone()),
            };
            imageops::replace(&mut canvas, &cell, (column * w) as i64, (row * h) as i64);
        }
    }
    RgbaImage {
        width,
        height,
        pixels: canvas.into_raw(),
    }
}

fn fnv64(bytes: &[u8]) -> String {
    let hash = bytes.iter().fold(0xcbf2_9ce4_8422_2325_u64, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(0x0100_0000_01b3)
    });
    format!("{hash:016x}")
}
