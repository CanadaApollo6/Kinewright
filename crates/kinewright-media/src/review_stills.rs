//! Deterministic review still sampling plan and bounded page decoding for the read-only Reviewer.

use std::{
    collections::{BTreeMap, BTreeSet},
    fs::File,
};

use kinewright_core::{
    BlendMode, Clip, ClipContent, ClipId, Document, ExactMicros, ExportCancellation, FrameRounding,
    MediaError, TimeCode, Title, TrackKind, map_frames_with_rounding,
    output_review::{OverlayPixels, OverlayText, ReviewExpectations, ReviewLane, ReviewSpan},
};
use serde::{Deserialize, Serialize};

use crate::{
    compositor::{params_for_at, pixel_coverage_edge},
    review_output::ReviewOutputTiming,
    review_overlays::{
        DecodedCardCache, ReviewArtifactVideoDecoder, ReviewReadSeek, decode_still_image_rgba8,
        expected_frame_pts, is_transformed, output_span_to_frames, quad_covers_pixel, quad_uv,
        sample_rgba_bilinear, transition_covers_pixel,
    },
    timeline::{title_alpha, transition_render_params},
    title::TitleRasterizer,
};

/// Maximum number of planned sample points for one review artifact.
pub const MAX_REVIEW_STILLS: usize = 32;
/// Maximum number of planned sample points served per `read_review_stills` page.
pub const MAX_STILLS_PER_PAGE: usize = 4;
/// Maximum width or height (pixels) of any served reviewer still.
pub const MAX_STILL_LONG_EDGE: u32 = 960;
/// Maximum byte length of one encoded PNG served to the reviewer.
pub const MAX_STILL_PNG_BYTES: usize = 512 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReviewStillReason {
    Opening,
    Middle,
    End,
    Cut,
    OverlayTransition,
    RepairedRegion,
    CommentedRegion,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReviewStillKind {
    DecodedOutput,
    OverlayReference,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ReviewStillPlanInput {
    /// Output-clock intervals repaired in this candidate.
    pub repaired_regions: Vec<ReviewSpan>,
    /// Output-clock intervals or anchor times commented on in Studio feedback.
    pub commented_regions: Vec<ReviewSpan>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlannedReviewStill {
    pub sample_index: u32,
    pub frame_index: u64,
    pub expected_pts: i64,
    pub output_micros: u64,
    pub reasons: Vec<ReviewStillReason>,
    /// Overlay elements expected visible on this frame (carries a reference still alongside decoded output).
    pub overlays: Vec<ClipId>,
}

impl PlannedReviewStill {
    #[must_use]
    pub fn expects_reference(&self) -> bool {
        !self.overlays.is_empty()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReviewStillPlan {
    pub total_frames: u64,
    pub candidate_count: usize,
    pub samples: Vec<PlannedReviewStill>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecodedReviewStill {
    pub sample_index: u32,
    pub frame_index: u64,
    pub pts: i64,
    pub output_micros: u64,
    pub kind: ReviewStillKind,
    pub reasons: Vec<ReviewStillReason>,
    pub width: u32,
    pub height: u32,
    pub rgba8: Vec<u8>,
}

/// Build the deterministic, bounded sample plan for independent visual review from frozen inputs.
///
/// Always includes opening, middle, end, video cuts, overlay transitions, and repaired/commented
/// regions, merged per frame and deterministically capped at [`MAX_REVIEW_STILLS`].
///
/// # Errors
/// Returns a media error when the timing map or expectation duration cannot produce sample points.
pub fn plan_review_stills(
    expectations: &ReviewExpectations,
    timing: &ReviewOutputTiming,
    extra: &ReviewStillPlanInput,
) -> Result<ReviewStillPlan, MediaError> {
    let total_frames = u64::try_from(expectations.output_frames.0).unwrap_or(0);
    if total_frames == 0 || !expectations.output_fps.is_valid() {
        return Err(MediaError::Backend(
            "review still plan requires a non-empty valid output timeline".into(),
        ));
    }
    if timing.video_frames < total_frames {
        return Err(MediaError::Backend(format!(
            "artifact video frame count {} is shorter than expected {total_frames}",
            timing.video_frames
        )));
    }

    let mut by_frame: BTreeMap<u64, BTreeSet<ReviewStillReason>> = BTreeMap::new();
    let mut add = |frame: u64, reason: ReviewStillReason| {
        let clamped = frame.min(total_frames - 1);
        by_frame.entry(clamped).or_default().insert(reason);
    };

    // 1. Opening, middle, end.
    add(0, ReviewStillReason::Opening);
    add(total_frames / 2, ReviewStillReason::Middle);
    add(total_frames - 1, ReviewStillReason::End);

    // 2. Cuts (video segment boundaries).
    for segment in &expectations.segments {
        if segment.id.lane != ReviewLane::Video {
            continue;
        }
        if let Some(frames) =
            output_span_to_frames(&segment.output, expectations.output_fps, total_frames)
            && !frames.is_empty()
        {
            if frames.start > 0 {
                add(frames.start, ReviewStillReason::Cut);
            }
            if frames.end < total_frames {
                add(frames.end - 1, ReviewStillReason::Cut);
            }
        }
    }

    // 3. Overlay transitions and active overlay tracking per frame.
    let overlay_runs =
        collect_overlay_transitions(expectations, total_frames, &mut |frame, reason| {
            add(frame, reason);
        });

    // 4. Repaired and commented regions.
    for span in &extra.repaired_regions {
        if let Some(frame) = span_midpoint_frame(span, expectations.output_fps, total_frames) {
            add(frame, ReviewStillReason::RepairedRegion);
        }
    }
    for span in &extra.commented_regions {
        if let Some(frame) = span_midpoint_frame(span, expectations.output_fps, total_frames) {
            add(frame, ReviewStillReason::CommentedRegion);
        }
    }

    let candidate_count = by_frame.len();
    let selected_frames = cap_candidate_frames(&by_frame, MAX_REVIEW_STILLS);

    let mut samples = Vec::with_capacity(selected_frames.len());
    for (idx, frame_index) in selected_frames.into_iter().enumerate() {
        let reasons: Vec<ReviewStillReason> = by_frame
            .get(&frame_index)
            .into_iter()
            .flatten()
            .copied()
            .collect();
        let expected_pts = expected_frame_pts(timing, frame_index).ok_or_else(|| {
            MediaError::Backend(format!(
                "cannot map review sample frame {frame_index} onto recorded PTS grid"
            ))
        })?;
        let output_micros = ExactMicros::frames(
            TimeCode(i64::try_from(frame_index).unwrap_or(i64::MAX)),
            expectations.output_fps,
        )
        .map_or(0, ExactMicros::floor);
        let mut overlays: Vec<ClipId> = overlay_runs
            .iter()
            .filter(|(_, range)| range.contains(&frame_index))
            .map(|(clip, _)| *clip)
            .collect();
        overlays.sort_unstable();
        overlays.dedup();
        samples.push(PlannedReviewStill {
            sample_index: u32::try_from(idx).unwrap_or(u32::MAX),
            frame_index,
            expected_pts,
            output_micros,
            reasons,
            overlays,
        });
    }

    Ok(ReviewStillPlan {
        total_frames,
        candidate_count,
        samples,
    })
}

fn collect_overlay_transitions(
    expectations: &ReviewExpectations,
    total_frames: u64,
    add: &mut dyn FnMut(u64, ReviewStillReason),
) -> Vec<(ClipId, std::ops::Range<u64>)> {
    let mut overlay_runs: Vec<(ClipId, std::ops::Range<u64>)> = Vec::new();
    for inventory in [
        &expectations.overlays.captions,
        &expectations.overlays.titles,
        &expectations.overlays.cards,
    ] {
        for overlay in inventory.known() {
            let mut first_start: Option<u64> = None;
            let mut last_end: Option<u64> = None;
            let mut run_count = 0_usize;
            for run in &overlay.visible {
                if let Some(frames) =
                    output_span_to_frames(&run.output, expectations.output_fps, total_frames)
                    && !frames.is_empty()
                {
                    first_start = Some(first_start.map_or(frames.start, |v| v.min(frames.start)));
                    last_end = Some(last_end.map_or(frames.end, |v| v.max(frames.end)));
                    overlay_runs.push((overlay.element, frames));
                    run_count += 1;
                }
            }
            if let (Some(start), Some(end)) = (first_start, last_end) {
                add(start, ReviewStillReason::OverlayTransition);
                if end > start + 1 {
                    add(end - 1, ReviewStillReason::OverlayTransition);
                }
                if end < total_frames {
                    add(end, ReviewStillReason::OverlayTransition);
                }
                if run_count > 1 {
                    add(
                        start + (end - start) / 2,
                        ReviewStillReason::OverlayTransition,
                    );
                }
            }
        }
    }
    overlay_runs
}

fn span_midpoint_frame(
    span: &ReviewSpan,
    output_fps: kinewright_core::Rational,
    total_frames: u64,
) -> Option<u64> {
    if span.start <= span.end {
        if span.start == span.end {
            let num = span
                .start
                .numerator()
                .checked_mul(u128::from(output_fps.numerator()))?;
            let den = 1_000_000_u128
                .checked_mul(u128::from(output_fps.denominator()))?
                .checked_mul(u128::from(span.start.denominator()))?;
            if den == 0 {
                return None;
            }
            let frame = u64::try_from(num / den).ok()?;
            return Some(frame.min(total_frames - 1));
        }
        let range = output_span_to_frames(span, output_fps, total_frames)?;
        if range.is_empty() {
            return Some(range.start.min(total_frames - 1));
        }
        return Some(range.start + (range.end - range.start) / 2);
    }
    None
}

fn cap_candidate_frames(
    by_frame: &BTreeMap<u64, BTreeSet<ReviewStillReason>>,
    cap: usize,
) -> Vec<u64> {
    if by_frame.len() <= cap {
        return by_frame.keys().copied().collect();
    }
    let mut chosen = BTreeSet::new();
    let tiers: [&[ReviewStillReason]; 4] = [
        &[
            ReviewStillReason::Opening,
            ReviewStillReason::Middle,
            ReviewStillReason::End,
        ],
        &[
            ReviewStillReason::RepairedRegion,
            ReviewStillReason::CommentedRegion,
        ],
        &[ReviewStillReason::OverlayTransition],
        &[ReviewStillReason::Cut],
    ];
    for tier in tiers {
        let remaining = cap.saturating_sub(chosen.len());
        if remaining == 0 {
            break;
        }
        let pool: Vec<u64> = by_frame
            .iter()
            .filter(|(frame, reasons)| {
                !chosen.contains(*frame) && reasons.iter().any(|r| tier.contains(r))
            })
            .map(|(&frame, _)| frame)
            .collect();
        if pool.len() <= remaining {
            chosen.extend(pool);
        } else {
            // Deterministically spread across the tier so early and late regions are covered.
            for slot in 0..remaining {
                let idx = if remaining == 1 {
                    0
                } else {
                    slot * (pool.len() - 1) / (remaining - 1)
                };
                chosen.insert(pool[idx]);
            }
        }
    }
    chosen.into_iter().collect()
}

/// Compute bounded still output dimensions preserving aspect ratio within [`MAX_STILL_LONG_EDGE`].
#[must_use]
pub fn bounded_still_dimensions(raster: (u32, u32), max_long_edge: u32) -> (u32, u32) {
    let (w, h) = raster;
    let long = w.max(h);
    let cap = max_long_edge.max(1);
    if long <= cap || w == 0 || h == 0 {
        return (w.max(1), h.max(1));
    }
    let scaled_w = (u64::from(w) * u64::from(cap))
        .div_ceil(u64::from(long))
        .max(1);
    let scaled_h = (u64::from(h) * u64::from(cap))
        .div_ceil(u64::from(long))
        .max(1);
    (
        u32::try_from(scaled_w).unwrap_or(cap),
        u32::try_from(scaled_h).unwrap_or(cap),
    )
}

/// Decode one bounded page of planned review stills (decoded artifact frames and, where overlays
/// are expected visible, frozen overlay reference stills). Holds at most one decoded frame at a
/// time while streaming to the target frames.
///
/// # Errors
/// Returns a media error when the artifact cannot be decoded, PTS disagrees with the recorded
/// grid, a requested frame is missing, or cancellation fires.
#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
pub fn decode_review_stills_page(
    artifact_file: File,
    timing: &ReviewOutputTiming,
    expectations: &ReviewExpectations,
    frozen_document: &Document,
    pinned_image_loader: &mut dyn FnMut(
        [u8; 32],
        u64,
    ) -> Result<Box<dyn ReviewReadSeek>, MediaError>,
    page_samples: &[PlannedReviewStill],
    max_long_edge: u32,
    cancellation: &ExportCancellation,
) -> Result<Vec<DecodedReviewStill>, MediaError> {
    if cancellation.is_cancelled() {
        return Err(MediaError::Cancelled);
    }
    if page_samples.is_empty() {
        return Ok(Vec::new());
    }
    if page_samples.len() > MAX_STILLS_PER_PAGE {
        return Err(MediaError::Backend(format!(
            "requested {} review stills exceeds page limit {MAX_STILLS_PER_PAGE}",
            page_samples.len()
        )));
    }
    let target_size = bounded_still_dimensions(
        expectations.raster,
        max_long_edge.clamp(1, MAX_STILL_LONG_EDGE),
    );
    let max_frame = page_samples
        .iter()
        .map(|s| s.frame_index)
        .max()
        .unwrap_or(0);
    let sample_by_frame: BTreeMap<u64, &PlannedReviewStill> = page_samples
        .iter()
        .map(|sample| (sample.frame_index, sample))
        .collect();

    let mut decoder =
        ReviewArtifactVideoDecoder::open(artifact_file, timing, target_size, cancellation)?;
    let mut decoded_by_sample: BTreeMap<u32, DecodedReviewStill> = BTreeMap::new();

    loop {
        if cancellation.is_cancelled() {
            return Err(MediaError::Cancelled);
        }
        let next_idx = decoder.next_frame_index();
        if next_idx > max_frame {
            break;
        }
        let want = sample_by_frame.get(&next_idx).copied();
        let Some(frame) = decoder.next_frame(want.is_some(), cancellation)? else {
            break;
        };
        if let Some(sample) = want {
            if frame.dimensions != expectations.raster {
                return Err(MediaError::Backend(format!(
                    "decoded review still raster {:?} disagrees with expected {:?}",
                    frame.dimensions, expectations.raster
                )));
            }
            if frame.pts != sample.expected_pts {
                return Err(MediaError::Backend(format!(
                    "decoded review still frame {} PTS {} disagrees with recorded {}",
                    frame.frame_index, frame.pts, sample.expected_pts
                )));
            }
            decoded_by_sample.insert(
                sample.sample_index,
                DecodedReviewStill {
                    sample_index: sample.sample_index,
                    frame_index: frame.frame_index,
                    pts: frame.pts,
                    output_micros: sample.output_micros,
                    kind: ReviewStillKind::DecodedOutput,
                    reasons: sample.reasons.clone(),
                    width: target_size.0,
                    height: target_size.1,
                    rgba8: frame.rgba8.to_vec(),
                },
            );
        }
    }

    let mut results = Vec::with_capacity(page_samples.len() * 2);
    let rasterizer = TitleRasterizer::new();
    let mut card_cache = DecodedCardCache::new();

    for sample in page_samples {
        let frame_still = decoded_by_sample
            .remove(&sample.sample_index)
            .ok_or_else(|| {
                MediaError::Backend(format!(
                    "review artifact ended before planned sample frame {}",
                    sample.frame_index
                ))
            })?;
        let pts = frame_still.pts;
        results.push(frame_still);

        if sample.expects_reference() {
            let reference = render_overlay_reference_still(
                sample,
                pts,
                target_size,
                expectations,
                frozen_document,
                &rasterizer,
                pinned_image_loader,
                &mut card_cache,
                cancellation,
            )?;
            results.push(reference);
        }
    }

    Ok(results)
}

#[allow(
    clippy::too_many_arguments,
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss
)]
fn render_overlay_reference_still(
    sample: &PlannedReviewStill,
    pts: i64,
    target_size: (u32, u32),
    expectations: &ReviewExpectations,
    frozen_document: &Document,
    rasterizer: &TitleRasterizer,
    pinned_image_loader: &mut dyn FnMut(
        [u8; 32],
        u64,
    ) -> Result<Box<dyn ReviewReadSeek>, MediaError>,
    card_cache: &mut DecodedCardCache,
    cancellation: &ExportCancellation,
) -> Result<DecodedReviewStill, MediaError> {
    let (w, h) = expectations.raster;
    let pixel_count = usize::try_from(w)
        .unwrap_or(0)
        .saturating_mul(usize::try_from(h).unwrap_or(0));
    let mut canvas = vec![0_u8; pixel_count.saturating_mul(4)];
    // Neutral dark slate checkerboard canvas so both light and dark overlays stand out clearly.
    for y in 0..h {
        for x in 0..w {
            let checker = ((x / 16) + (y / 16)) % 2 == 0;
            let shade = if checker { 24_u8 } else { 32_u8 };
            let off = (y as usize * w as usize + x as usize) * 4;
            canvas[off..off + 4].copy_from_slice(&[shade, shade, shade + 4, 255]);
        }
    }

    let output_tc = TimeCode(i64::try_from(sample.frame_index).unwrap_or(i64::MAX));
    let project_at = map_frames_with_rounding(
        output_tc,
        expectations.output_fps,
        expectations.project_fps,
        FrameRounding::Floor,
    )
    .map_err(|error| MediaError::Backend(error.to_string()))?;

    // Composite expected overlays in document track order, before any occluding non-overlay layers.
    for track in frozen_document
        .tracks
        .iter()
        .filter(|track| track.kind == TrackKind::Video)
    {
        for clip in &track.clips {
            if !sample.overlays.contains(&clip.id) {
                continue;
            }
            if cancellation.is_cancelled() {
                return Err(MediaError::Cancelled);
            }
            let Some(overlay) = expectations
                .overlays
                .captions
                .known()
                .chain(expectations.overlays.titles.known())
                .chain(expectations.overlays.cards.known())
                .find(|entry| entry.element == clip.id)
            else {
                continue;
            };
            composite_expected_overlay(
                &mut canvas,
                (w, h),
                clip,
                overlay,
                project_at,
                frozen_document,
                rasterizer,
                pinned_image_loader,
                card_cache,
                cancellation,
            )?;
        }
    }

    let rgba8 = if target_size == (w, h) {
        canvas
    } else {
        downscale_rgba8(&canvas, (w, h), target_size)
    };

    Ok(DecodedReviewStill {
        sample_index: sample.sample_index,
        frame_index: sample.frame_index,
        pts,
        output_micros: sample.output_micros,
        kind: ReviewStillKind::OverlayReference,
        reasons: sample.reasons.clone(),
        width: target_size.0,
        height: target_size.1,
        rgba8,
    })
}

#[allow(
    clippy::too_many_arguments,
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss
)]
fn composite_expected_overlay(
    canvas: &mut [u8],
    resolution: (u32, u32),
    clip: &Clip,
    overlay: &kinewright_core::output_review::ReviewOverlay,
    project_at: TimeCode,
    frozen_document: &Document,
    rasterizer: &TitleRasterizer,
    pinned_image_loader: &mut dyn FnMut(
        [u8; 32],
        u64,
    ) -> Result<Box<dyn ReviewReadSeek>, MediaError>,
    card_cache: &mut DecodedCardCache,
    cancellation: &ExportCancellation,
) -> Result<(), MediaError> {
    if clip.blend_mode != BlendMode::Normal {
        return Ok(());
    }
    let local = TimeCode(project_at.0.saturating_sub(clip.timeline_start.0));
    let mut transition = transition_render_params(clip, project_at);
    let (src_w, src_h, src_rgba) = match overlay.pixels {
        OverlayPixels::RenderedText => {
            let OverlayText::Expected(text) = &overlay.text else {
                return Ok(());
            };
            let ClipContent::Title(clip_title) = &clip.content else {
                return Ok(());
            };
            let expected_title = Title {
                text: text.clone(),
                ..clip_title.clone()
            };
            transition.alpha *= title_alpha(frozen_document, clip, &expected_title, project_at)?;
            let rendered = rasterizer.rasterize_with_glyph_mask(&expected_title, resolution)?;
            (rendered.width, rendered.height, rendered.rgba)
        }
        OverlayPixels::PinnedImage {
            sha256,
            byte_length,
        } => {
            if let Some(cached) = card_cache.get(&(sha256, byte_length)) {
                cached.clone()
            } else {
                let reader = pinned_image_loader(sha256, byte_length)?;
                let decoded = decode_still_image_rgba8(reader, cancellation)?;
                card_cache.insert((sha256, byte_length), decoded.clone());
                decoded
            }
        }
        OverlayPixels::Unknown(_) => return Ok(()),
    };

    let (w, h) = resolution;
    let mut params = params_for_at(&clip.effects, transition, Some(local));
    params.frame_aspect = h as f32 / w as f32;
    if params.coverage_on > 0.5 {
        pixel_coverage_edge(&mut params, w, h);
    }
    if params.opacity <= 0.0 || params.scale_x == 0.0 || params.scale_y == 0.0 {
        return Ok(());
    }
    let extent = [w as f32, h as f32];
    let transformed = is_transformed(&params) || (src_w, src_h) != resolution;

    for y in 0..h as usize {
        for x in 0..w as usize {
            if !quad_covers_pixel(&params, extent, [x, y])
                || !transition_covers_pixel(&params, extent, [x, y])
            {
                continue;
            }
            let src = if transformed {
                let ndc = [
                    (x as f32 + 0.5) / extent[0] * 2.0 - 1.0,
                    1.0 - (y as f32 + 0.5) / extent[1] * 2.0,
                ];
                let uv = quad_uv(&params, ndc);
                if uv[0] < 0.0 || uv[0] > 1.0 || uv[1] < 0.0 || uv[1] > 1.0 {
                    continue;
                }
                sample_rgba_bilinear(&src_rgba, src_w, src_h, uv)
            } else {
                let off = (y * w as usize + x) * 4;
                [
                    src_rgba[off],
                    src_rgba[off + 1],
                    src_rgba[off + 2],
                    src_rgba[off + 3],
                ]
            };
            let eff_a = ((f32::from(src[3]) / 255.0) * params.opacity).clamp(0.0, 1.0);
            if eff_a <= 0.0 {
                continue;
            }
            let dst_off = (y * w as usize + x) * 4;
            for c in 0..3 {
                let blended =
                    f32::from(src[c]) * eff_a + f32::from(canvas[dst_off + c]) * (1.0 - eff_a);
                canvas[dst_off + c] = blended.round().clamp(0.0, 255.0) as u8;
            }
            canvas[dst_off + 3] = 255;
        }
    }
    Ok(())
}

#[allow(clippy::cast_precision_loss)]
fn downscale_rgba8(src: &[u8], src_size: (u32, u32), dst_size: (u32, u32)) -> Vec<u8> {
    let (dw, dh) = dst_size;
    let mut out = Vec::with_capacity(dw as usize * dh as usize * 4);
    for y in 0..dh {
        let v = (y as f32 + 0.5) / dh as f32;
        for x in 0..dw {
            let u = (x as f32 + 0.5) / dw as f32;
            let px = sample_rgba_bilinear(src, src_size.0, src_size.1, [u, v]);
            out.extend_from_slice(&px);
        }
    }
    out
}

#[cfg(test)]
mod tests;
