//! Deterministic review still sampling plan and bounded seekable page decoding for the
//! read-only Reviewer.

use std::{
    collections::{BTreeMap, BTreeSet},
    fs::File,
    sync::Arc,
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
/// Maximum `read_review_stills` page calls allowed per reviewer attempt.
pub const MAX_STILL_PAGES_PER_ATTEMPT: usize = 16;
/// Cap on dropped sample point entries stored in [`ReviewStillPlan::dropped_samples`].
pub const MAX_DROPPED_SAMPLES_RECORDED: usize = 128;
/// Cap on uncovered region entries stored in [`ReviewStillPlan::uncovered_regions`].
pub const MAX_UNCOVERED_REGIONS_RECORDED: usize = 64;
/// Seek directly to the next sample on a page when the frame gap exceeds this threshold.
const SEEK_GAP_THRESHOLD_FRAMES: u64 = 48;

/// Fixed, validated configuration governing reviewer still planning, paging, and PNG caps.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReviewStillsConfig {
    pub max_stills: usize,
    pub max_stills_per_page: usize,
    pub max_long_edge: u32,
    pub max_png_bytes: usize,
    pub max_pages_per_attempt: usize,
}

impl Default for ReviewStillsConfig {
    fn default() -> Self {
        Self {
            max_stills: MAX_REVIEW_STILLS,
            max_stills_per_page: MAX_STILLS_PER_PAGE,
            max_long_edge: MAX_STILL_LONG_EDGE,
            max_png_bytes: MAX_STILL_PNG_BYTES,
            max_pages_per_attempt: MAX_STILL_PAGES_PER_ATTEMPT,
        }
    }
}

impl ReviewStillsConfig {
    /// Validate that every still cap is strictly positive and within hard ceilings.
    ///
    /// # Errors
    /// Returns [`MediaError::Backend`] when any limit is zero or exceeds its hard cap.
    pub fn validate(&self) -> Result<(), MediaError> {
        if self.max_stills == 0 || self.max_stills > MAX_REVIEW_STILLS {
            return Err(MediaError::Backend(format!(
                "invalid review stills config: max_stills={} must be in 1..={MAX_REVIEW_STILLS}",
                self.max_stills
            )));
        }
        if self.max_stills_per_page == 0 || self.max_stills_per_page > MAX_STILLS_PER_PAGE {
            return Err(MediaError::Backend(format!(
                "invalid review stills config: max_stills_per_page={} must be in 1..={MAX_STILLS_PER_PAGE}",
                self.max_stills_per_page
            )));
        }
        if self.max_long_edge < 64 || self.max_long_edge > MAX_STILL_LONG_EDGE {
            return Err(MediaError::Backend(format!(
                "invalid review stills config: max_long_edge={} must be in 64..={MAX_STILL_LONG_EDGE}",
                self.max_long_edge
            )));
        }
        if self.max_png_bytes < 4096 || self.max_png_bytes > MAX_STILL_PNG_BYTES {
            return Err(MediaError::Backend(format!(
                "invalid review stills config: max_png_bytes={} must be in 4096..={MAX_STILL_PNG_BYTES}",
                self.max_png_bytes
            )));
        }
        if self.max_pages_per_attempt == 0
            || self.max_pages_per_attempt > MAX_STILL_PAGES_PER_ATTEMPT
        {
            return Err(MediaError::Backend(format!(
                "invalid review stills config: max_pages_per_attempt={} must be in 1..={MAX_STILL_PAGES_PER_ATTEMPT}",
                self.max_pages_per_attempt
            )));
        }
        Ok(())
    }

    /// Stable canonical SHA-256 digest of this reviewer-stills configuration, ready to be
    /// combined with other review-check digests into one `ReviewCheckBinding`.
    #[must_use]
    pub fn sha256(&self) -> [u8; 32] {
        let mut hasher = kinewright_hash::Sha256::new();
        hasher.update(b"kinewright-review-stills-config-v2\0");
        hasher.update(&(u64::try_from(self.max_stills).unwrap_or(u64::MAX)).to_le_bytes());
        hasher.update(&(u64::try_from(self.max_stills_per_page).unwrap_or(u64::MAX)).to_le_bytes());
        hasher.update(&self.max_long_edge.to_le_bytes());
        hasher.update(&(u64::try_from(self.max_png_bytes).unwrap_or(u64::MAX)).to_le_bytes());
        hasher
            .update(&(u64::try_from(self.max_pages_per_attempt).unwrap_or(u64::MAX)).to_le_bytes());
        hasher.finalize()
    }
}

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

/// A commented or repaired region that could not be read or mapped onto the frozen candidate's
/// output clock. Any uncovered region keeps the visual review row from passing (L12).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UncoveredStillRegion {
    pub reason: ReviewStillReason,
    pub detail: String,
}

/// A candidate sample point omitted by the plan's still cap (Opus m4).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DroppedReviewStill {
    pub frame_index: u64,
    pub output_micros: u64,
    pub reasons: Vec<ReviewStillReason>,
}

/// An overlay that could not be composited into an [`ReviewStillKind::OverlayReference`] still
/// (Opus m3). Never dropped silently.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OmittedReferenceOverlay {
    pub element: ClipId,
    pub reason: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ReviewStillPlanInput {
    /// Output-clock intervals repaired in this candidate.
    pub repaired_regions: Vec<ReviewSpan>,
    /// Output-clock intervals or anchor times commented on in Studio feedback.
    pub commented_regions: Vec<ReviewSpan>,
    /// Commented or repaired regions that could not be read or mapped onto this candidate's clock.
    pub uncovered_regions: Vec<UncoveredStillRegion>,
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
    /// Total candidate points dropped by `config.max_stills`.
    pub dropped_count: usize,
    /// Number of dropped candidates that carried `RepairedRegion` or `CommentedRegion`.
    pub dropped_repaired_or_commented: usize,
    /// Bounded list of candidate points dropped by `config.max_stills`.
    pub dropped_samples: Vec<DroppedReviewStill>,
    /// Bounded list of commented/repaired regions that could not be read or mapped onto this cut.
    pub uncovered_regions: Vec<UncoveredStillRegion>,
    pub uncovered_regions_total: usize,
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
    /// Non-empty when an `OverlayReference` still could not render one or more planned overlays.
    pub omitted_overlays: Vec<OmittedReferenceOverlay>,
}

/// Decoded stills plus bounded decode telemetry for one `decode_review_stills_page` call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecodedReviewStillsPage {
    pub stills: Vec<DecodedReviewStill>,
    pub frames_decoded: u64,
    pub seeks_performed: u64,
}

/// Build the deterministic, bounded sample plan for independent visual review from frozen inputs.
///
/// Always includes opening, middle, end, video cuts, overlay transitions, and repaired/commented
/// regions, merged per frame and deterministically capped at `config.max_stills`. Any candidate
/// dropped by the cap is recorded in `dropped_samples`; any unmappable/unreadable region is
/// recorded in `uncovered_regions`.
///
/// # Errors
/// Returns a media error when `config` is invalid, the timing map lacks a positive frame step,
/// or expectation duration cannot produce sample points.
#[allow(clippy::too_many_lines)]
pub fn plan_review_stills(
    expectations: &ReviewExpectations,
    timing: &ReviewOutputTiming,
    extra: &ReviewStillPlanInput,
    config: &ReviewStillsConfig,
) -> Result<ReviewStillPlan, MediaError> {
    config.validate()?;
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
    if timing.video_pts_step.is_none_or(|step| step <= 0) {
        return Err(MediaError::Backend(
            "review still plan requires a positive recorded video PTS step".into(),
        ));
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

    // 4. Repaired and commented regions (out-of-range/invalid spans become uncovered, never dropped).
    let mut uncovered_regions = extra.uncovered_regions.clone();
    for span in &extra.repaired_regions {
        if let Some(frame) = span_midpoint_frame(span, expectations.output_fps, total_frames) {
            add(frame, ReviewStillReason::RepairedRegion);
        } else {
            uncovered_regions.push(UncoveredStillRegion {
                reason: ReviewStillReason::RepairedRegion,
                detail: "repaired output span could not be mapped to an output frame".into(),
            });
        }
    }
    for span in &extra.commented_regions {
        if let Some(frame) = span_midpoint_frame(span, expectations.output_fps, total_frames) {
            add(frame, ReviewStillReason::CommentedRegion);
        } else {
            uncovered_regions.push(UncoveredStillRegion {
                reason: ReviewStillReason::CommentedRegion,
                detail: "commented output span could not be mapped to an output frame".into(),
            });
        }
    }

    let candidate_count = by_frame.len();
    let selected_frames = cap_candidate_frames(&by_frame, config.max_stills);
    let selected_set: BTreeSet<u64> = selected_frames.iter().copied().collect();

    let mut dropped_count = 0_usize;
    let mut dropped_repaired_or_commented = 0_usize;
    let mut dropped_samples = Vec::new();
    for (&frame_index, reasons_set) in &by_frame {
        if selected_set.contains(&frame_index) {
            continue;
        }
        dropped_count = dropped_count.saturating_add(1);
        let reasons: Vec<ReviewStillReason> = reasons_set.iter().copied().collect();
        if reasons.iter().any(|r| {
            matches!(
                r,
                ReviewStillReason::RepairedRegion | ReviewStillReason::CommentedRegion
            )
        }) {
            dropped_repaired_or_commented = dropped_repaired_or_commented.saturating_add(1);
        }
        if dropped_samples.len() < MAX_DROPPED_SAMPLES_RECORDED {
            let output_micros = ExactMicros::frames(
                TimeCode(i64::try_from(frame_index).unwrap_or(i64::MAX)),
                expectations.output_fps,
            )
            .map_or(0, ExactMicros::floor);
            dropped_samples.push(DroppedReviewStill {
                frame_index,
                output_micros,
                reasons,
            });
        }
    }

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

    let uncovered_regions_total = uncovered_regions.len();
    uncovered_regions.truncate(MAX_UNCOVERED_REGIONS_RECORDED);

    Ok(ReviewStillPlan {
        total_frames,
        candidate_count,
        samples,
        dropped_count,
        dropped_repaired_or_commented,
        dropped_samples,
        uncovered_regions,
        uncovered_regions_total,
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
            if frame >= total_frames {
                return None;
            }
            return Some(frame);
        }
        let range = output_span_to_frames(span, output_fps, total_frames)?;
        if range.is_empty() {
            return None;
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

/// Compute bounded still output dimensions preserving aspect ratio within `max_long_edge`.
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
/// are expected visible, frozen overlay reference stills). Seeks directly to the keyframe before
/// each page's first sample (and across large gaps) rather than decoding from frame 0, holding at
/// most one decoded frame at a time.
///
/// # Errors
/// Returns a media error when `config` is invalid, the artifact cannot be decoded or sought, PTS
/// disagrees with the recorded grid, a requested frame is missing, or cancellation fires.
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
    config: &ReviewStillsConfig,
    cancellation: &ExportCancellation,
) -> Result<DecodedReviewStillsPage, MediaError> {
    if cancellation.is_cancelled() {
        return Err(MediaError::Cancelled);
    }
    config.validate()?;
    if page_samples.is_empty() {
        return Ok(DecodedReviewStillsPage {
            stills: Vec::new(),
            frames_decoded: 0,
            seeks_performed: 0,
        });
    }
    if page_samples.len() > config.max_stills_per_page {
        return Err(MediaError::Backend(format!(
            "requested {} review stills exceeds configured page limit {}",
            page_samples.len(),
            config.max_stills_per_page
        )));
    }
    let target_size = bounded_still_dimensions(expectations.raster, config.max_long_edge);

    let mut ordered_samples: Vec<&PlannedReviewStill> = page_samples.iter().collect();
    ordered_samples.sort_unstable_by_key(|s| s.frame_index);

    let mut decoder =
        ReviewArtifactVideoDecoder::open(artifact_file, timing, target_size, cancellation)?;
    let mut decoded_by_sample: BTreeMap<u32, DecodedReviewStill> = BTreeMap::new();
    let mut cursor_frame: Option<u64> = None;

    for sample in &ordered_samples {
        if cancellation.is_cancelled() {
            return Err(MediaError::Cancelled);
        }
        let target = sample.frame_index;
        let should_seek = match cursor_frame {
            None => target > 0,
            Some(current) => {
                target < current || target.saturating_sub(current) > SEEK_GAP_THRESHOLD_FRAMES
            }
        };
        let mut retried_from_start = false;
        if should_seek {
            decoder.seek_to_frame(target, timing, cancellation)?;
        }

        loop {
            if cancellation.is_cancelled() {
                return Err(MediaError::Cancelled);
            }
            let Some(frame) =
                decoder.next_frame_by_pts(timing, |idx| idx == target, cancellation)?
            else {
                return Err(MediaError::Backend(format!(
                    "review artifact ended before planned sample frame {target}"
                )));
            };
            cursor_frame = Some(frame.frame_index.saturating_add(1));

            if frame.frame_index > target {
                if !retried_from_start {
                    retried_from_start = true;
                    decoder.seek_to_start(cancellation)?;
                    continue;
                }
                return Err(MediaError::Backend(format!(
                    "review artifact skipped planned sample frame {target} (decoded frame {})",
                    frame.frame_index
                )));
            }
            if frame.frame_index < target {
                continue;
            }
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
                    omitted_overlays: Vec::new(),
                },
            );
            break;
        }
    }

    let frames_decoded = decoder.frames_decoded();
    let seeks_performed = decoder.seeks_performed();
    drop(decoder);

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

    Ok(DecodedReviewStillsPage {
        stills: results,
        frames_decoded,
        seeks_performed,
    })
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

    let mut omitted_overlays = Vec::new();
    let mut seen_clips = BTreeSet::new();

    // Composite expected overlays in document track order, recording any overlay that cannot be
    // drawn rather than silently omitting it (Opus m3).
    for track in frozen_document
        .tracks
        .iter()
        .filter(|track| track.kind == TrackKind::Video)
    {
        for clip in &track.clips {
            if !sample.overlays.contains(&clip.id) {
                continue;
            }
            seen_clips.insert(clip.id);
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
                omitted_overlays.push(OmittedReferenceOverlay {
                    element: clip.id,
                    reason: "overlay missing from frozen expectation record".into(),
                });
                continue;
            };
            if let Some(reason) = composite_expected_overlay(
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
            )? {
                omitted_overlays.push(OmittedReferenceOverlay {
                    element: clip.id,
                    reason,
                });
            }
        }
    }

    for &planned_clip in &sample.overlays {
        if !seen_clips.contains(&planned_clip) {
            omitted_overlays.push(OmittedReferenceOverlay {
                element: planned_clip,
                reason: "planned overlay clip missing from frozen document".into(),
            });
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
        omitted_overlays,
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
) -> Result<Option<String>, MediaError> {
    if clip.blend_mode != BlendMode::Normal {
        return Ok(Some(format!(
            "unsupported blend mode {:?} for standalone overlay reference",
            clip.blend_mode
        )));
    }
    let local = TimeCode(project_at.0.saturating_sub(clip.timeline_start.0));
    let mut transition = transition_render_params(clip, project_at);
    let (src_w, src_h, src_rgba): (u32, u32, Arc<Vec<u8>>) = match overlay.pixels {
        OverlayPixels::RenderedText => {
            let OverlayText::Expected(text) = &overlay.text else {
                return Ok(Some("overlay expected text is unresolved".into()));
            };
            let ClipContent::Title(clip_title) = &clip.content else {
                return Ok(Some(
                    "rendered text overlay does not reference a Title clip".into(),
                ));
            };
            let expected_title = Title {
                text: text.clone(),
                ..clip_title.clone()
            };
            transition.alpha *= title_alpha(frozen_document, clip, &expected_title, project_at)?;
            let rendered = rasterizer.rasterize(&expected_title, resolution)?;
            (rendered.width, rendered.height, rendered.rgba)
        }
        OverlayPixels::PinnedImage {
            sha256,
            byte_length,
        } => {
            if let Some((cw, ch, cached)) = card_cache.get(&(sha256, byte_length)) {
                (*cw, *ch, Arc::clone(cached))
            } else {
                let reader = pinned_image_loader(sha256, byte_length)?;
                let decoded = decode_still_image_rgba8(reader, cancellation)?;
                card_cache.insert(
                    (sha256, byte_length),
                    (decoded.0, decoded.1, Arc::clone(&decoded.2)),
                );
                decoded
            }
        }
        OverlayPixels::Unknown(reason) => {
            return Ok(Some(format!("overlay pixels unresolved: {reason:?}")));
        }
    };

    let (w, h) = resolution;
    let mut params = params_for_at(&clip.effects, transition, Some(local));
    params.frame_aspect = h as f32 / w as f32;
    if params.coverage_on > 0.5 {
        pixel_coverage_edge(&mut params, w, h);
    }
    if params.opacity <= 0.0 || params.scale_x == 0.0 || params.scale_y == 0.0 {
        return Ok(Some(
            "overlay has zero effective opacity or scale on sample frame".into(),
        ));
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
    Ok(None)
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
