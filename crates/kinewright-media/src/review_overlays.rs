//! Pure decoded-region verification of captions, titles and pinned cards against frozen review expectations.
//!
//! Reads only the verified artifact stream, frozen expectations, frozen document clip styling/animation,
//! and verified pinned card images. Holds at most one decoded video frame in memory at a time.

use std::{collections::BTreeMap, fs::File, ops::Range, path::Path};

use ffmpeg_next as ffmpeg;
use kinewright_core::{
    BlendMode, Clip, ClipContent, ClipId, ColorMatrix, ColorRange, Document, ExactMicros,
    ExportCancellation, FrameRounding, MediaError, Rational, TimeCode, Title, TitlePixelBounds,
    TrackKind, map_frames_with_rounding,
    output_review::{
        OverlayKind, OverlayPixels, OverlayText, ReviewExpectations, ReviewOverlay, ReviewRowResult,
    },
};
use serde::{Deserialize, Serialize};

use crate::{
    color_pipeline::{decode_bt709, encode_bt709},
    compositor::{LayerParams, params_for_at, pixel_coverage_edge},
    decode::{backend, color_description_from_decoder, managed_filter_graph},
    review_output::ReviewOutputTiming,
    timeline::{title_alpha, transition_render_params},
    title::{RasterizedTitleOverlay, TitleRasterizer},
};

/// Cap on findings retained in one [`OverlayVerificationReport`].
pub const MAX_OVERLAY_FINDINGS: usize = 64;
/// Maximum output frame index scanned during overlay verification.
pub const MAX_OVERLAY_FRAME_SCAN: u64 = 1_000_000;
/// Bounded decode window: one decoded video frame resident at a time.
pub const OVERLAY_DECODE_WINDOW_FRAMES: usize = 1;

const MIN_SEGMENT_CORE_PIXELS: usize = 6;
const MIN_GLYPH_BOX_NON_GLYPH_PIXELS: usize = 8;
const SCRIM_MAX_LUMA_OVER_WHITE: f32 = 134.0;
const OUTLINE_MAX_LUMA_OVER_WHITE: f32 = 86.0;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OverlayVerificationConfig {
    /// Allowed BT.709 luma (Y, 0..=255 scale) difference on decoded overlay pixels.
    pub luma_tolerance: f32,
    /// Allowed BT.709 chroma (Cb/Cr, 0..=255 scale) difference (absorbs 4:2:0 subsampling on thin strokes).
    pub chroma_tolerance: f32,
    /// Minimum fraction of expected solid glyph core pixels matching expected rendered color.
    pub min_glyph_match_ratio: f32,
    /// Minimum normalized correlation between expected glyph coverage and decoded foreground contrast.
    pub min_glyph_correlation: f32,
    /// Minimum localized recall / correlation per word/glyph group (catches single-word/digit substitution).
    pub min_local_group_correlation: f32,
    /// Maximum fraction of eroded non-glyph interior pixels matching foreground color.
    pub max_extra_ink_ratio: f32,
    /// Minimum fraction of opaque pinned-image card pixels matching expected image within tolerance.
    pub min_card_match_ratio: f32,
    /// Minimum effective opacity above which partial-opacity contrast is also checked alongside the linear envelope.
    pub min_measurable_opacity: f32,
    /// Maximum number of findings recorded in the report (clamped to [`MAX_OVERLAY_FINDINGS`]).
    pub max_findings: usize,
}

impl Default for OverlayVerificationConfig {
    fn default() -> Self {
        Self {
            luma_tolerance: 38.0,
            chroma_tolerance: 56.0,
            min_glyph_match_ratio: 0.78,
            min_glyph_correlation: 0.62,
            min_local_group_correlation: 0.48,
            max_extra_ink_ratio: 0.12,
            min_card_match_ratio: 0.85,
            min_measurable_opacity: 0.35,
            max_findings: MAX_OVERLAY_FINDINGS,
        }
    }
}

impl OverlayVerificationConfig {
    /// Stable canonical SHA-256 digest of this overlay verification configuration, ready to be
    /// combined with other review-check digests into one `ReviewCheckBinding`.
    #[must_use]
    pub fn sha256(&self) -> [u8; 32] {
        let mut hasher = kinewright_hash::Sha256::new();
        hasher.update(b"kinewright-overlay-verification-config-v1\0");
        for value in [
            self.luma_tolerance,
            self.chroma_tolerance,
            self.min_glyph_match_ratio,
            self.min_glyph_correlation,
            self.min_local_group_correlation,
            self.max_extra_ink_ratio,
            self.min_card_match_ratio,
            self.min_measurable_opacity,
        ] {
            let normalized = if value == 0.0 { 0.0_f32 } else { value };
            hasher.update(&normalized.to_bits().to_le_bytes());
        }
        let max_findings = u64::try_from(self.max_findings.clamp(1, MAX_OVERLAY_FINDINGS))
            .unwrap_or(MAX_OVERLAY_FINDINGS as u64);
        hasher.update(&max_findings.to_le_bytes());
        hasher.finalize()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OverlayElementStatus {
    Passed,
    Failed,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OverlayElementSummary {
    pub element: ClipId,
    pub kind: OverlayKind,
    pub status: OverlayElementStatus,
    pub expected_frames: u64,
    pub checked_frames: u64,
    pub animated_extreme_frames: u64,
    pub failed_frames: u64,
    pub min_recall: f32,
    pub max_extra_ink: f32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum OverlayFindingKind {
    UnresolvedExpectation {
        reason: String,
    },
    StreamUnreadable {
        reason: String,
    },
    RasterMismatch {
        expected: (u32, u32),
        observed: (u32, u32),
    },
    TimingMismatch {
        expected_pts: i64,
        observed_pts: i64,
    },
    MissingFrame {
        expected_frames: u64,
        observed_frames: u64,
    },
    BoundsMismatch {
        expected: TitlePixelBounds,
    },
    GlyphMismatch {
        recall_percent: u8,
        worst_glyph_recall_percent: u8,
        extra_ink_percent: u8,
    },
    ScrimOrOutlineMismatch {
        violation_percent: u8,
    },
    FadeEnvelopeMismatch {
        match_percent: u8,
    },
    PinnedCardMismatch {
        match_percent: u8,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OverlayFinding {
    pub element: Option<ClipId>,
    pub overlay_kind: Option<OverlayKind>,
    pub frame_index: Option<u64>,
    pub pts: Option<i64>,
    pub detail: OverlayFindingKind,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OverlayVerificationReport {
    pub row_result: ReviewRowResult,
    pub checked_overlays: usize,
    pub checked_frames: u64,
    pub animated_extreme_frames: u64,
    pub total_findings: usize,
    pub findings_truncated: bool,
    pub findings: Vec<OverlayFinding>,
    pub overlays: Vec<OverlayElementSummary>,
}

impl OverlayVerificationReport {
    fn new(max_findings: usize) -> (Self, usize) {
        let cap = max_findings.clamp(1, MAX_OVERLAY_FINDINGS);
        (
            Self {
                row_result: ReviewRowResult::Unknown,
                checked_overlays: 0,
                checked_frames: 0,
                animated_extreme_frames: 0,
                total_findings: 0,
                findings_truncated: false,
                findings: Vec::new(),
                overlays: Vec::new(),
            },
            cap,
        )
    }

    fn push_finding(&mut self, cap: usize, finding: OverlayFinding) {
        self.total_findings = self.total_findings.saturating_add(1);
        if self.findings.len() < cap {
            self.findings.push(finding);
        } else {
            self.findings_truncated = true;
        }
    }
}

/// Verified readable/seekable byte stream used to load frozen pinned overlay cards without
/// widening file-system access.
pub trait ReviewReadSeek: std::io::Read + std::io::Seek + Send {}
impl<T: std::io::Read + std::io::Seek + Send> ReviewReadSeek for T {}

pub(crate) type DecodedCardCache = BTreeMap<([u8; 32], u64), (u32, u32, Vec<u8>)>;

/// Verify every expected caption, title and pinned card on every frame of its visible range
/// against the decoded artifact stream, using the shared title rasteriser on the frozen text
/// before any occluding layers.
///
/// # Errors
/// Returns [`MediaError::Cancelled`] when `cancellation` fires; stream/format/expectation
/// problems that prevent deciding are reported in the returned report as
/// [`ReviewRowResult::Unknown`] (or [`ReviewRowResult::Failed`] for decoded-region defects).
#[allow(clippy::too_many_lines)]
pub fn verify_review_overlays(
    artifact_file: File,
    artifact_timing: &ReviewOutputTiming,
    expectations: &ReviewExpectations,
    frozen_document: &Document,
    pinned_image_loader: &mut dyn FnMut(
        [u8; 32],
        u64,
    ) -> Result<Box<dyn ReviewReadSeek>, MediaError>,
    config: &OverlayVerificationConfig,
    cancellation: &ExportCancellation,
) -> Result<OverlayVerificationReport, MediaError> {
    if cancellation.is_cancelled() {
        return Err(MediaError::Cancelled);
    }
    let (mut report, finding_cap) = OverlayVerificationReport::new(config.max_findings);

    if expectations.raster.0 == 0
        || expectations.raster.1 == 0
        || !expectations.output_fps.is_valid()
        || !expectations.project_fps.is_valid()
        || expectations.output_frames.0 <= 0
    {
        report.push_finding(
            finding_cap,
            OverlayFinding {
                element: None,
                overlay_kind: None,
                frame_index: None,
                pts: None,
                detail: OverlayFindingKind::UnresolvedExpectation {
                    reason: "invalid output raster, frame rate or duration in review expectations"
                        .into(),
                },
            },
        );
        report.row_result = ReviewRowResult::Unknown;
        return Ok(report);
    }

    let inventories = [
        (OverlayKind::Caption, &expectations.overlays.captions),
        (OverlayKind::Title, &expectations.overlays.titles),
        (OverlayKind::Card, &expectations.overlays.cards),
    ];

    let mut blocked = false;
    for (kind, inventory) in inventories {
        if !inventory.is_determined() {
            blocked = true;
            report.push_finding(
                finding_cap,
                OverlayFinding {
                    element: None,
                    overlay_kind: Some(kind),
                    frame_index: None,
                    pts: None,
                    detail: OverlayFindingKind::UnresolvedExpectation {
                        reason: format!("{kind:?} overlay inventory is not determined"),
                    },
                },
            );
        }
    }
    if !expectations.overlays.is_resolved() {
        for (kind, inventory) in inventories {
            for overlay in inventory.known() {
                if overlay.visible.is_empty() {
                    report.push_finding(
                        finding_cap,
                        OverlayFinding {
                            element: Some(overlay.element),
                            overlay_kind: Some(kind),
                            frame_index: None,
                            pts: None,
                            detail: OverlayFindingKind::UnresolvedExpectation {
                                reason: "overlay has no visible ranges".into(),
                            },
                        },
                    );
                }
                if let OverlayPixels::Unknown(reason) = overlay.pixels {
                    report.push_finding(
                        finding_cap,
                        OverlayFinding {
                            element: Some(overlay.element),
                            overlay_kind: Some(kind),
                            frame_index: None,
                            pts: None,
                            detail: OverlayFindingKind::UnresolvedExpectation {
                                reason: format!("overlay pixels unresolved: {reason:?}"),
                            },
                        },
                    );
                }
                if matches!(overlay.pixels, OverlayPixels::RenderedText) {
                    match &overlay.text {
                        OverlayText::Expected(text) if !text.trim().is_empty() => {}
                        other => {
                            report.push_finding(
                                finding_cap,
                                OverlayFinding {
                                    element: Some(overlay.element),
                                    overlay_kind: Some(kind),
                                    frame_index: None,
                                    pts: None,
                                    detail: OverlayFindingKind::UnresolvedExpectation {
                                        reason: format!("rendered text unresolved: {other:?}"),
                                    },
                                },
                            );
                        }
                    }
                }
                for run in &overlay.visible {
                    if let Err(unknown) = run.bounds {
                        report.push_finding(
                            finding_cap,
                            OverlayFinding {
                                element: Some(overlay.element),
                                overlay_kind: Some(kind),
                                frame_index: None,
                                pts: None,
                                detail: OverlayFindingKind::UnresolvedExpectation {
                                    reason: format!("overlay bounds unresolved: {unknown:?}"),
                                },
                            },
                        );
                    }
                }
            }
        }
        blocked = true;
    }

    if blocked {
        report.row_result = ReviewRowResult::Unknown;
        return Ok(report);
    }

    let rasterizer = TitleRasterizer::new();
    let mut card_cache = DecodedCardCache::new();
    let mut prepared: Vec<PreparedOverlay<'_>> = Vec::new();
    let mut max_needed_frame: Option<u64> = None;

    for (kind, inventory) in inventories {
        for overlay in inventory.known() {
            if cancellation.is_cancelled() {
                return Err(MediaError::Cancelled);
            }
            match prepare_overlay(
                overlay,
                kind,
                expectations,
                frozen_document,
                &rasterizer,
                pinned_image_loader,
                &mut card_cache,
                cancellation,
            ) {
                Ok(entry) => {
                    if let Some(last) = entry.last_frame() {
                        max_needed_frame =
                            Some(max_needed_frame.map_or(last, |prev| prev.max(last)));
                    }
                    prepared.push(entry);
                }
                Err(PrepareOverlayError::Cancelled) => return Err(MediaError::Cancelled),
                Err(PrepareOverlayError::Blocked(reason)) => {
                    blocked = true;
                    report.push_finding(
                        finding_cap,
                        OverlayFinding {
                            element: Some(overlay.element),
                            overlay_kind: Some(kind),
                            frame_index: None,
                            pts: None,
                            detail: OverlayFindingKind::UnresolvedExpectation { reason },
                        },
                    );
                }
            }
        }
    }

    if blocked {
        report.row_result = ReviewRowResult::Unknown;
        return Ok(report);
    }

    let mut decoder = match ReviewArtifactVideoDecoder::open(
        artifact_file,
        artifact_timing,
        expectations.raster,
        cancellation,
    ) {
        Ok(decoder) => decoder,
        Err(MediaError::Cancelled) => return Err(MediaError::Cancelled),
        Err(error) => {
            report.push_finding(
                finding_cap,
                OverlayFinding {
                    element: None,
                    overlay_kind: None,
                    frame_index: None,
                    pts: None,
                    detail: OverlayFindingKind::StreamUnreadable {
                        reason: error.to_string(),
                    },
                },
            );
            report.row_result = ReviewRowResult::Unknown;
            return Ok(report);
        }
    };

    // Even when all three overlay inventories are NoneProven, verify the stream actually decodes
    // at least one frame at the expected raster so an unreadable stream never passes (L2).
    if prepared.is_empty() {
        match decoder.next_frame(false, cancellation) {
            Ok(Some(frame)) => {
                if frame.dimensions != expectations.raster {
                    report.push_finding(
                        finding_cap,
                        OverlayFinding {
                            element: None,
                            overlay_kind: None,
                            frame_index: Some(frame.frame_index),
                            pts: Some(frame.pts),
                            detail: OverlayFindingKind::RasterMismatch {
                                expected: expectations.raster,
                                observed: frame.dimensions,
                            },
                        },
                    );
                    report.row_result = ReviewRowResult::Failed;
                    return Ok(report);
                }
                report.row_result = ReviewRowResult::Pass;
                return Ok(report);
            }
            Ok(None) => {
                report.push_finding(
                    finding_cap,
                    OverlayFinding {
                        element: None,
                        overlay_kind: None,
                        frame_index: Some(0),
                        pts: None,
                        detail: OverlayFindingKind::StreamUnreadable {
                            reason: "decoded artifact yielded zero video frames".into(),
                        },
                    },
                );
                report.row_result = ReviewRowResult::Unknown;
                return Ok(report);
            }
            Err(MediaError::Cancelled) => return Err(MediaError::Cancelled),
            Err(error) => {
                report.push_finding(
                    finding_cap,
                    OverlayFinding {
                        element: None,
                        overlay_kind: None,
                        frame_index: Some(0),
                        pts: None,
                        detail: OverlayFindingKind::StreamUnreadable {
                            reason: error.to_string(),
                        },
                    },
                );
                report.row_result = ReviewRowResult::Unknown;
                return Ok(report);
            }
        }
    }

    let target_last = max_needed_frame.unwrap_or(0);
    let mut active_indices = Vec::with_capacity(prepared.len());
    let mut failed = false;

    loop {
        if cancellation.is_cancelled() {
            return Err(MediaError::Cancelled);
        }
        let next_index = decoder.next_frame_index();
        if next_index > target_last {
            break;
        }
        active_indices.clear();
        for (idx, overlay) in prepared.iter().enumerate() {
            if overlay.run_at(next_index).is_some() {
                active_indices.push(idx);
            }
        }
        let need_pixels = !active_indices.is_empty();
        let frame = match decoder.next_frame(need_pixels, cancellation) {
            Ok(Some(frame)) => frame,
            Ok(None) => break,
            Err(MediaError::Cancelled) => return Err(MediaError::Cancelled),
            Err(error) => {
                report.push_finding(
                    finding_cap,
                    OverlayFinding {
                        element: None,
                        overlay_kind: None,
                        frame_index: Some(next_index),
                        pts: None,
                        detail: OverlayFindingKind::StreamUnreadable {
                            reason: error.to_string(),
                        },
                    },
                );
                report.row_result = ReviewRowResult::Unknown;
                return Ok(report);
            }
        };

        if frame.dimensions != expectations.raster {
            failed = true;
            report.push_finding(
                finding_cap,
                OverlayFinding {
                    element: None,
                    overlay_kind: None,
                    frame_index: Some(frame.frame_index),
                    pts: Some(frame.pts),
                    detail: OverlayFindingKind::RasterMismatch {
                        expected: expectations.raster,
                        observed: frame.dimensions,
                    },
                },
            );
            break;
        }

        if let Some(expected_pts) = expected_frame_pts(artifact_timing, frame.frame_index)
            && frame.pts != expected_pts
        {
            failed = true;
            report.push_finding(
                finding_cap,
                OverlayFinding {
                    element: None,
                    overlay_kind: None,
                    frame_index: Some(frame.frame_index),
                    pts: Some(frame.pts),
                    detail: OverlayFindingKind::TimingMismatch {
                        expected_pts,
                        observed_pts: frame.pts,
                    },
                },
            );
        }

        if !need_pixels {
            continue;
        }

        for &idx in &active_indices {
            let overlay = &mut prepared[idx];
            let Some(bounds) = overlay.run_at(frame.frame_index) else {
                continue;
            };
            let is_extreme = overlay.is_extreme_frame(frame.frame_index);
            overlay.checked_frames = overlay.checked_frames.saturating_add(1);
            report.checked_frames = report.checked_frames.saturating_add(1);
            if is_extreme {
                overlay.checked_extreme_frames = overlay.checked_extreme_frames.saturating_add(1);
                report.animated_extreme_frames = report.animated_extreme_frames.saturating_add(1);
            }
            let outcome = check_overlay_on_frame(
                overlay,
                frame.frame_index,
                frame.pts,
                bounds,
                frame.rgba8,
                expectations,
                frozen_document,
                config,
            );
            match outcome {
                FrameCheckOutcome::Pass { recall, extra_ink } => {
                    overlay.min_recall = overlay.min_recall.min(recall);
                    overlay.max_extra_ink = overlay.max_extra_ink.max(extra_ink);
                }
                FrameCheckOutcome::Fail {
                    recall,
                    extra_ink,
                    detail,
                } => {
                    failed = true;
                    overlay.failed_frames = overlay.failed_frames.saturating_add(1);
                    overlay.min_recall = overlay.min_recall.min(recall);
                    overlay.max_extra_ink = overlay.max_extra_ink.max(extra_ink);
                    report.push_finding(
                        finding_cap,
                        OverlayFinding {
                            element: Some(overlay.element),
                            overlay_kind: Some(overlay.kind),
                            frame_index: Some(frame.frame_index),
                            pts: Some(frame.pts),
                            detail,
                        },
                    );
                }
                FrameCheckOutcome::Blocked(reason) => {
                    blocked = true;
                    overlay.blocked = true;
                    report.push_finding(
                        finding_cap,
                        OverlayFinding {
                            element: Some(overlay.element),
                            overlay_kind: Some(overlay.kind),
                            frame_index: Some(frame.frame_index),
                            pts: Some(frame.pts),
                            detail: OverlayFindingKind::UnresolvedExpectation { reason },
                        },
                    );
                }
            }
        }
    }

    for overlay in &prepared {
        let missing = overlay.checked_frames < overlay.expected_frames;
        if missing {
            failed = true;
            report.push_finding(
                finding_cap,
                OverlayFinding {
                    element: Some(overlay.element),
                    overlay_kind: Some(overlay.kind),
                    frame_index: None,
                    pts: None,
                    detail: OverlayFindingKind::MissingFrame {
                        expected_frames: overlay.expected_frames,
                        observed_frames: overlay.checked_frames,
                    },
                },
            );
        }
        let status = if overlay.blocked {
            OverlayElementStatus::Unknown
        } else if overlay.failed_frames > 0 || missing {
            OverlayElementStatus::Failed
        } else {
            OverlayElementStatus::Passed
        };
        report.overlays.push(OverlayElementSummary {
            element: overlay.element,
            kind: overlay.kind,
            status,
            expected_frames: overlay.expected_frames,
            checked_frames: overlay.checked_frames,
            animated_extreme_frames: overlay.checked_extreme_frames,
            failed_frames: overlay.failed_frames,
            min_recall: if overlay.checked_frames > 0 {
                overlay.min_recall
            } else {
                0.0
            },
            max_extra_ink: overlay.max_extra_ink,
        });
    }

    report.checked_overlays = prepared.len();
    report.row_result = if failed {
        ReviewRowResult::Failed
    } else if blocked {
        ReviewRowResult::Unknown
    } else {
        ReviewRowResult::Pass
    };
    Ok(report)
}

pub(crate) fn expected_frame_pts(timing: &ReviewOutputTiming, frame_index: u64) -> Option<i64> {
    if frame_index == 0 {
        return Some(timing.video_first_pts);
    }
    let step = timing.video_pts_step?;
    let offset = i64::try_from(frame_index).ok()?.checked_mul(step)?;
    timing.video_first_pts.checked_add(offset)
}

/// Convert a half-open [`ExactMicros`] span on `output_fps` into exact half-open output frame indices.
pub(crate) fn output_span_to_frames(
    span: &Range<ExactMicros>,
    output_fps: Rational,
    total_frames: u64,
) -> Option<Range<u64>> {
    if !output_fps.is_valid() || span.start >= span.end {
        return None;
    }
    let to_frame_ceil = |time: ExactMicros| -> Option<u64> {
        let num = time
            .numerator()
            .checked_mul(u128::from(output_fps.numerator()))?;
        let den = 1_000_000_u128
            .checked_mul(u128::from(output_fps.denominator()))?
            .checked_mul(u128::from(time.denominator()))?;
        if den == 0 {
            return None;
        }
        u64::try_from(num.div_ceil(den)).ok()
    };
    let start = to_frame_ceil(span.start)?.min(total_frames);
    let end = to_frame_ceil(span.end)?.min(total_frames);
    Some(start..end)
}

enum PrepareOverlayError {
    Cancelled,
    Blocked(String),
}

enum PreparedOverlayContent {
    Text {
        title: Title,
        raster: RasterizedTitleOverlay,
    },
    PinnedCard {
        width: u32,
        height: u32,
        rgba8: Vec<u8>,
    },
}

struct FrameBoundsRun {
    frames: Range<u64>,
    bounds: TitlePixelBounds,
}

struct PreparedOverlay<'a> {
    element: ClipId,
    kind: OverlayKind,
    clip: &'a Clip,
    content: PreparedOverlayContent,
    runs: Vec<FrameBoundsRun>,
    extreme_frames: Vec<u64>,
    expected_frames: u64,
    checked_frames: u64,
    checked_extreme_frames: u64,
    failed_frames: u64,
    blocked: bool,
    min_recall: f32,
    max_extra_ink: f32,
}

impl PreparedOverlay<'_> {
    fn last_frame(&self) -> Option<u64> {
        self.runs
            .iter()
            .filter(|run| !run.frames.is_empty())
            .map(|run| run.frames.end - 1)
            .max()
    }

    fn run_at(&self, frame_index: u64) -> Option<TitlePixelBounds> {
        let pos = self
            .runs
            .partition_point(|run| run.frames.end <= frame_index);
        let run = self.runs.get(pos)?;
        run.frames.contains(&frame_index).then_some(run.bounds)
    }

    fn is_extreme_frame(&self, frame_index: u64) -> bool {
        self.extreme_frames.binary_search(&frame_index).is_ok()
    }
}

#[allow(clippy::too_many_arguments)]
fn prepare_overlay<'a>(
    overlay: &ReviewOverlay,
    kind: OverlayKind,
    expectations: &ReviewExpectations,
    frozen_document: &'a Document,
    rasterizer: &TitleRasterizer,
    pinned_image_loader: &mut dyn FnMut(
        [u8; 32],
        u64,
    ) -> Result<Box<dyn ReviewReadSeek>, MediaError>,
    card_cache: &mut DecodedCardCache,
    cancellation: &ExportCancellation,
) -> Result<PreparedOverlay<'a>, PrepareOverlayError> {
    let clip = frozen_document
        .tracks
        .iter()
        .filter(|track| track.kind == TrackKind::Video)
        .flat_map(|track| &track.clips)
        .find(|clip| clip.id == overlay.element)
        .ok_or_else(|| {
            PrepareOverlayError::Blocked(format!(
                "frozen document is missing overlay clip {}",
                overlay.element
            ))
        })?;
    if clip.blend_mode != BlendMode::Normal {
        return Err(PrepareOverlayError::Blocked(format!(
            "overlay clip {} uses non-normal blend mode {:?}",
            overlay.element, clip.blend_mode
        )));
    }
    if let Some(effect) = clip.effects.iter().find(|effect| {
        (effect.enabled || effect.enabled_curve.is_some())
            && !matches!(effect.name.as_str(), "transform" | "opacity")
    }) {
        return Err(PrepareOverlayError::Blocked(format!(
            "overlay clip {} has unsupported active effect {}",
            overlay.element, effect.name
        )));
    }

    let total_frames = u64::try_from(expectations.output_frames.0).unwrap_or(0);
    let mut runs = Vec::with_capacity(overlay.visible.len());
    let mut expected_frames: u64 = 0;
    for run in &overlay.visible {
        let bounds = run.bounds.map_err(|unknown| {
            PrepareOverlayError::Blocked(format!("unresolved bounds: {unknown:?}"))
        })?;
        let frames = output_span_to_frames(&run.output, expectations.output_fps, total_frames)
            .ok_or_else(|| PrepareOverlayError::Blocked("invalid visible output span".into()))?;
        if frames.end > MAX_OVERLAY_FRAME_SCAN {
            return Err(PrepareOverlayError::Blocked(format!(
                "overlay range end {} exceeds scan bound {MAX_OVERLAY_FRAME_SCAN}",
                frames.end
            )));
        }
        expected_frames = expected_frames.saturating_add(frames.end.saturating_sub(frames.start));
        runs.push(FrameBoundsRun { frames, bounds });
    }
    if expected_frames == 0 {
        return Err(PrepareOverlayError::Blocked(
            "overlay visible range covers zero output frames".into(),
        ));
    }
    runs.sort_unstable_by_key(|run| run.frames.start);

    let extreme_frames = compute_extreme_frames(&runs);
    let content = prepare_overlay_content(
        overlay,
        clip,
        expectations.raster,
        rasterizer,
        pinned_image_loader,
        card_cache,
        cancellation,
    )?;

    Ok(PreparedOverlay {
        element: overlay.element,
        kind,
        clip,
        content,
        runs,
        extreme_frames,
        expected_frames,
        checked_frames: 0,
        checked_extreme_frames: 0,
        failed_frames: 0,
        blocked: false,
        min_recall: 1.0,
        max_extra_ink: 0.0,
    })
}

fn compute_extreme_frames(runs: &[FrameBoundsRun]) -> Vec<u64> {
    let mut extreme_frames = Vec::new();
    if let (Some(first), Some(last)) = (runs.first(), runs.last()) {
        if !first.frames.is_empty() {
            extreme_frames.push(first.frames.start);
        }
        if !last.frames.is_empty() {
            extreme_frames.push(last.frames.end - 1);
        }
    }
    if runs.len() > 1 {
        let mut min_left = (i32::MAX, 0_u64);
        let mut max_right = (i32::MIN, 0_u64);
        let mut min_top = (i32::MAX, 0_u64);
        let mut max_bottom = (i32::MIN, 0_u64);
        for run in runs {
            if run.frames.is_empty() {
                continue;
            }
            let f = run.frames.start;
            if run.bounds.left < min_left.0 {
                min_left = (run.bounds.left, f);
            }
            if run.bounds.right > max_right.0 {
                max_right = (run.bounds.right, f);
            }
            if run.bounds.top < min_top.0 {
                min_top = (run.bounds.top, f);
            }
            if run.bounds.bottom > max_bottom.0 {
                max_bottom = (run.bounds.bottom, f);
            }
        }
        extreme_frames.extend([min_left.1, max_right.1, min_top.1, max_bottom.1]);
    }
    extreme_frames.sort_unstable();
    extreme_frames.dedup();
    extreme_frames
}

fn prepare_overlay_content(
    overlay: &ReviewOverlay,
    clip: &Clip,
    raster: (u32, u32),
    rasterizer: &TitleRasterizer,
    pinned_image_loader: &mut dyn FnMut(
        [u8; 32],
        u64,
    ) -> Result<Box<dyn ReviewReadSeek>, MediaError>,
    card_cache: &mut DecodedCardCache,
    cancellation: &ExportCancellation,
) -> Result<PreparedOverlayContent, PrepareOverlayError> {
    match overlay.pixels {
        OverlayPixels::RenderedText => {
            let OverlayText::Expected(expected_text) = &overlay.text else {
                return Err(PrepareOverlayError::Blocked(
                    "rendered text overlay lacks expected text".into(),
                ));
            };
            let ClipContent::Title(clip_title) = &clip.content else {
                return Err(PrepareOverlayError::Blocked(format!(
                    "expected title clip for {}, found non-title content",
                    overlay.element
                )));
            };
            if clip_title.text != *expected_text {
                return Err(PrepareOverlayError::Blocked(format!(
                    "frozen document text for {} disagrees with review expectations",
                    overlay.element
                )));
            }
            let expected_title = Title {
                text: expected_text.clone(),
                ..clip_title.clone()
            };
            let raster = rasterizer
                .rasterize_with_glyph_mask(&expected_title, raster)
                .map_err(|error| {
                    PrepareOverlayError::Blocked(format!("title rasterization failed: {error}"))
                })?;
            if !raster.glyph_alpha.iter().any(|&a| a >= 64) {
                return Err(PrepareOverlayError::Blocked(format!(
                    "frozen text for {} rasterized to zero visible glyph pixels",
                    overlay.element
                )));
            }
            Ok(PreparedOverlayContent::Text {
                title: expected_title,
                raster,
            })
        }
        OverlayPixels::PinnedImage {
            sha256,
            byte_length,
        } => {
            let (width, height, rgba8) =
                if let Some(cached) = card_cache.get(&(sha256, byte_length)) {
                    cached.clone()
                } else {
                    let reader =
                        pinned_image_loader(sha256, byte_length).map_err(|error| match error {
                            MediaError::Cancelled => PrepareOverlayError::Cancelled,
                            other => PrepareOverlayError::Blocked(format!(
                                "pinned card image for {} unavailable: {other}",
                                overlay.element
                            )),
                        })?;
                    let decoded = decode_still_image_rgba8(reader, cancellation).map_err(
                        |error| match error {
                            MediaError::Cancelled => PrepareOverlayError::Cancelled,
                            other => PrepareOverlayError::Blocked(format!(
                                "pinned card image for {} could not be decoded: {other}",
                                overlay.element
                            )),
                        },
                    )?;
                    card_cache.insert((sha256, byte_length), decoded.clone());
                    decoded
                };
            Ok(PreparedOverlayContent::PinnedCard {
                width,
                height,
                rgba8,
            })
        }
        OverlayPixels::Unknown(reason) => Err(PrepareOverlayError::Blocked(format!(
            "unresolved pixels: {reason:?}"
        ))),
    }
}

enum FrameCheckOutcome {
    Pass {
        recall: f32,
        extra_ink: f32,
    },
    Fail {
        recall: f32,
        extra_ink: f32,
        detail: OverlayFindingKind,
    },
    Blocked(String),
}

#[allow(
    clippy::too_many_arguments,
    clippy::too_many_lines,
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_possible_wrap
)]
fn check_overlay_on_frame(
    overlay: &PreparedOverlay<'_>,
    frame_index: u64,
    _pts: i64,
    bounds: TitlePixelBounds,
    decoded_rgba8: &[u8],
    expectations: &ReviewExpectations,
    frozen_document: &Document,
    config: &OverlayVerificationConfig,
) -> FrameCheckOutcome {
    let (width, height) = expectations.raster;
    let w_i32 = i32::try_from(width).unwrap_or(i32::MAX);
    let h_i32 = i32::try_from(height).unwrap_or(i32::MAX);
    if bounds.left < 0
        || bounds.top < 0
        || bounds.right > w_i32
        || bounds.bottom > h_i32
        || bounds.left >= bounds.right
        || bounds.top >= bounds.bottom
    {
        return FrameCheckOutcome::Fail {
            recall: 0.0,
            extra_ink: 0.0,
            detail: OverlayFindingKind::BoundsMismatch { expected: bounds },
        };
    }

    let Ok(output_tc) = i64::try_from(frame_index).map(TimeCode) else {
        return FrameCheckOutcome::Blocked("frame index overflow".into());
    };
    let Ok(project_at) = map_frames_with_rounding(
        output_tc,
        expectations.output_fps,
        expectations.project_fps,
        FrameRounding::Floor,
    ) else {
        return FrameCheckOutcome::Blocked("output-to-project frame mapping failed".into());
    };
    let local = TimeCode(project_at.0.saturating_sub(overlay.clip.timeline_start.0));
    let mut transition = transition_render_params(overlay.clip, project_at);
    if let PreparedOverlayContent::Text { title, .. } = &overlay.content {
        match title_alpha(frozen_document, overlay.clip, title, project_at) {
            Ok(alpha) => transition.alpha *= alpha,
            Err(error) => return FrameCheckOutcome::Blocked(error.to_string()),
        }
    }
    let mut params = params_for_at(&overlay.clip.effects, transition, Some(local));
    params.frame_aspect = height as f32 / width as f32;
    if params.coverage_on > 0.5 {
        pixel_coverage_edge(&mut params, width, height);
    }

    if params.opacity <= 0.0 || params.scale_x == 0.0 || params.scale_y == 0.0 {
        return FrameCheckOutcome::Fail {
            recall: 0.0,
            extra_ink: 0.0,
            detail: OverlayFindingKind::BoundsMismatch { expected: bounds },
        };
    }

    match &overlay.content {
        PreparedOverlayContent::Text { raster, .. } => check_text_overlay_on_frame(
            raster,
            &params,
            bounds,
            (width, height),
            decoded_rgba8,
            config,
        ),
        PreparedOverlayContent::PinnedCard {
            width: src_w,
            height: src_h,
            rgba8: card_rgba8,
        } => check_card_overlay_on_frame(
            (*src_w, *src_h),
            card_rgba8,
            &params,
            bounds,
            (width, height),
            decoded_rgba8,
            config,
        ),
    }
}

#[allow(
    clippy::too_many_lines,
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_possible_wrap,
    clippy::float_cmp
)]
fn check_text_overlay_on_frame(
    raster: &RasterizedTitleOverlay,
    params: &LayerParams,
    bounds: TitlePixelBounds,
    resolution: (u32, u32),
    decoded_rgba8: &[u8],
    config: &OverlayVerificationConfig,
) -> FrameCheckOutcome {
    let (width, height) = resolution;
    let bw = usize::try_from(bounds.right - bounds.left).unwrap_or(0);
    let bh = usize::try_from(bounds.bottom - bounds.top).unwrap_or(0);
    let area = bw.saturating_mul(bh);
    if area == 0 {
        return FrameCheckOutcome::Fail {
            recall: 0.0,
            extra_ink: 0.0,
            detail: OverlayFindingKind::BoundsMismatch { expected: bounds },
        };
    }

    let transformed = is_transformed(params);
    let mut proj_glyph = vec![0_u8; area];
    let mut proj_layer = vec![0_u8; area];
    let mut proj_rgba = vec![[0_u8; 4]; area];
    let mut in_text_box = vec![false; area];
    let mut in_scrim_inner = vec![false; area];
    let mut max_glyph_alpha: u8 = 0;

    let extent = [width as f32, height as f32];
    for ly in 0..bh {
        let y = bounds.top + ly as i32;
        for lx in 0..bw {
            let x = bounds.left + lx as i32;
            let idx = ly * bw + lx;
            if transformed {
                let pixel = [x as usize, y as usize];
                if !quad_covers_pixel(params, extent, pixel)
                    || !transition_covers_pixel(params, extent, pixel)
                {
                    continue;
                }
                let ndc = [
                    (x as f32 + 0.5) / extent[0] * 2.0 - 1.0,
                    1.0 - (y as f32 + 0.5) / extent[1] * 2.0,
                ];
                let uv = quad_uv(params, ndc);
                if uv[0] < 0.0 || uv[0] > 1.0 || uv[1] < 0.0 || uv[1] > 1.0 {
                    continue;
                }
                let g = sample_mask_bilinear(&raster.glyph_alpha, raster.width, raster.height, uv);
                let px = sample_rgba_bilinear(&raster.rgba, raster.width, raster.height, uv);
                proj_glyph[idx] = g;
                proj_layer[idx] = px[3];
                proj_rgba[idx] = px;
                max_glyph_alpha = max_glyph_alpha.max(g);
                let sx = uv[0] * raster.width as f32;
                let sy = uv[1] * raster.height as f32;
                in_text_box[idx] = sx >= (raster.text_bounds.left - 1) as f32
                    && sx < (raster.text_bounds.right + 1) as f32
                    && sy >= (raster.text_bounds.top - 1) as f32
                    && sy < (raster.text_bounds.bottom + 1) as f32;
                in_scrim_inner[idx] = raster.scrim_active
                    && sx >= (raster.visual_bounds.left + 2) as f32
                    && sx < (raster.visual_bounds.right - 2) as f32
                    && sy >= (raster.visual_bounds.top + 2) as f32
                    && sy < (raster.visual_bounds.bottom - 2) as f32;
            } else {
                if params.coverage_on > 0.5
                    && !transition_covers_pixel(params, extent, [x as usize, y as usize])
                {
                    continue;
                }
                let src_idx = y as usize * width as usize + x as usize;
                let g = raster.glyph_alpha[src_idx];
                let off = src_idx * 4;
                let px = [
                    raster.rgba[off],
                    raster.rgba[off + 1],
                    raster.rgba[off + 2],
                    raster.rgba[off + 3],
                ];
                proj_glyph[idx] = g;
                proj_layer[idx] = px[3];
                proj_rgba[idx] = px;
                max_glyph_alpha = max_glyph_alpha.max(g);
                in_text_box[idx] = x >= raster.text_bounds.left - 1
                    && x < raster.text_bounds.right + 1
                    && y >= raster.text_bounds.top - 1
                    && y < raster.text_bounds.bottom + 1;
                in_scrim_inner[idx] = raster.scrim_active
                    && x >= raster.visual_bounds.left + 2
                    && x < raster.visual_bounds.right - 2
                    && y >= raster.visual_bounds.top + 2
                    && y < raster.visual_bounds.bottom - 2;
            }
        }
    }

    if max_glyph_alpha < 64 {
        // E.g. clipped completely by a wipe transition on this frame before any text enters:
        // verify scrim if any is visible, otherwise nothing to reject on this frame.
        if !proj_layer.iter().any(|&a| a >= 64) {
            return FrameCheckOutcome::Pass {
                recall: 1.0,
                extra_ink: 0.0,
            };
        }
        return check_scrim_only_frame(
            &proj_layer,
            &in_scrim_inner,
            params.opacity,
            bounds,
            resolution,
            decoded_rgba8,
            config,
        );
    }

    let core_threshold = ((u16::from(max_glyph_alpha) * 3) / 4).clamp(112, 180) as u8;
    let fg_rgb = [raster.fg_rgba[0], raster.fg_rgba[1], raster.fg_rgba[2]];
    let fg_ycbcr = rgb_to_ycbcr(fg_rgb);
    let dilate_radius: isize = if transformed { 3 } else { 2 };

    // Build dilated glyph exclusion mask so anti-aliased edges and codec ringing are not
    // miscounted as extra ink.
    let mut near_glyph = vec![false; area];
    for ly in 0..bh {
        for lx in 0..bw {
            if proj_glyph[ly * bw + lx] < 24 {
                continue;
            }
            let y0 = (ly as isize - dilate_radius).max(0) as usize;
            let y1 = (ly as isize + dilate_radius).min(bh as isize - 1) as usize;
            let x0 = (lx as isize - dilate_radius).max(0) as usize;
            let x1 = (lx as isize + dilate_radius).min(bw as isize - 1) as usize;
            for ny in y0..=y1 {
                for nx in x0..=x1 {
                    near_glyph[ny * bw + nx] = true;
                }
            }
        }
    }

    if params.opacity < 0.85 {
        return check_partial_opacity_text_frame(
            &proj_glyph,
            &proj_layer,
            &in_text_box,
            &in_scrim_inner,
            &near_glyph,
            core_threshold,
            fg_rgb,
            raster.scrim_active,
            params.opacity,
            bounds,
            resolution,
            decoded_rgba8,
            config,
        );
    }

    // Estimate local non-glyph backdrop color inside the overlay box so partial-coverage
    // thin strokes (e.g. 1px stems at small delivery rasters) compare against the true
    // rendered color rather than assuming 100% opaque foreground.
    let mut bg_sum = [0.0_f64; 3];
    let mut bg_count = 0_usize;
    let mut scrim_total: usize = 0;
    let mut scrim_violations: usize = 0;
    let mut outline_total: usize = 0;
    let mut outline_violations: usize = 0;
    let mut non_glyph_total: usize = 0;
    let mut extra_ink: usize = 0;

    for ly in 0..bh {
        let y = bounds.top + ly as i32;
        for lx in 0..bw {
            let x = bounds.left + lx as i32;
            let idx = ly * bw + lx;
            if near_glyph[idx] {
                continue;
            }
            let dec = decoded_rgb_at(decoded_rgba8, resolution, x, y);
            let dec_ycbcr = rgb_to_ycbcr(dec);
            if in_scrim_inner[idx] {
                scrim_total += 1;
                if dec_ycbcr[0] > SCRIM_MAX_LUMA_OVER_WHITE + config.luma_tolerance {
                    scrim_violations += 1;
                }
                non_glyph_total += 1;
                if ycbcr_matches(
                    dec_ycbcr,
                    fg_ycbcr,
                    config.luma_tolerance,
                    config.chroma_tolerance,
                ) {
                    extra_ink += 1;
                }
                for c in 0..3 {
                    bg_sum[c] += f64::from(dec[c]);
                }
                bg_count += 1;
            } else if in_text_box[idx] {
                non_glyph_total += 1;
                if ycbcr_matches(
                    dec_ycbcr,
                    fg_ycbcr,
                    config.luma_tolerance,
                    config.chroma_tolerance,
                ) {
                    extra_ink += 1;
                }
                for c in 0..3 {
                    bg_sum[c] += f64::from(dec[c]);
                }
                bg_count += 1;
            }
            if raster.outline_active && proj_layer[idx] >= 180 && proj_glyph[idx] == 0 {
                outline_total += 1;
                if dec_ycbcr[0] > OUTLINE_MAX_LUMA_OVER_WHITE + config.luma_tolerance + 12.0 {
                    outline_violations += 1;
                }
            }
        }
    }

    let bg_rgb = if bg_count > 0 {
        [
            (bg_sum[0] / bg_count as f64) as f32,
            (bg_sum[1] / bg_count as f64) as f32,
            (bg_sum[2] / bg_count as f64) as f32,
        ]
    } else {
        [16.0, 16.0, 16.0]
    };
    let bg_lin = bg_rgb.map(|c| decode_bt709((c / 255.0).clamp(0.0, 1.0)));

    // Precompute the decoded foreground-alignment field D(x, y) over the text box:
    // projection of (decoded - bg_rgb) onto (fg_rgb - bg_rgb) in RGB space, used for
    // normalized correlation against the expected glyph mask.
    let diff_vec = [
        f32::from(fg_rgb[0]) - bg_rgb[0],
        f32::from(fg_rgb[1]) - bg_rgb[1],
        f32::from(fg_rgb[2]) - bg_rgb[2],
    ];
    let diff_norm_sq =
        diff_vec[0] * diff_vec[0] + diff_vec[1] * diff_vec[1] + diff_vec[2] * diff_vec[2];

    let search_radius: i32 = 1;
    let mut core_total: usize = 0;
    let mut core_matched: usize = 0;
    let mut corr_global = PearsonAccum::default();
    let mut fg_signal = vec![0.0_f32; area];

    for ly in 0..bh {
        let y = bounds.top + ly as i32;
        for lx in 0..bw {
            let x = bounds.left + lx as i32;
            let idx = ly * bw + lx;
            let dec = decoded_rgb_at(decoded_rgba8, resolution, x, y);
            let sig = if diff_norm_sq > 100.0 {
                let d = [
                    f32::from(dec[0]) - bg_rgb[0],
                    f32::from(dec[1]) - bg_rgb[1],
                    f32::from(dec[2]) - bg_rgb[2],
                ];
                ((d[0] * diff_vec[0] + d[1] * diff_vec[1] + d[2] * diff_vec[2]) / diff_norm_sq)
                    .clamp(-0.5, 1.5)
            } else {
                0.0
            };
            fg_signal[idx] = sig;

            if in_text_box[idx] {
                corr_global.push(f64::from(proj_glyph[idx]) / 255.0, f64::from(sig));
            }

            if proj_glyph[idx] >= core_threshold {
                core_total += 1;
                // Expected rendered RGB at this pixel combines the rasterized overlay pixel
                // (including partial anti-aliased glyph coverage over scrim/outline) with the
                // local backdrop via scene-linear alpha compositing.
                let expected_px = expected_composited_rgb(proj_rgba[idx], bg_lin, params.opacity);
                let exp_ycbcr = rgb_to_ycbcr(expected_px);
                // Also accept a match to full fg_ycbcr (in case a neighbor pixel hits the exact
                // peak of a thin stroke).
                let matched_here = pixel_matches_either_in_neighborhood(
                    decoded_rgba8,
                    resolution,
                    x,
                    y,
                    search_radius,
                    exp_ycbcr,
                    fg_ycbcr,
                    config.luma_tolerance,
                    config.chroma_tolerance,
                ) || (diff_norm_sq > 900.0 && sig >= 0.45);
                if matched_here {
                    core_matched += 1;
                }
            }
        }
    }

    if core_total == 0 {
        return FrameCheckOutcome::Pass {
            recall: 1.0,
            extra_ink: 0.0,
        };
    }

    let recall = core_matched as f32 / core_total as f32;
    let global_corr = corr_global.correlation().unwrap_or(0.0) as f32;
    let extra_ink_ratio = if non_glyph_total >= MIN_GLYPH_BOX_NON_GLYPH_PIXELS {
        extra_ink as f32 / non_glyph_total as f32
    } else {
        0.0
    };

    // Group adjacent glyphs into word-scale boxes so even small delivery fonts have enough
    // pixels per group while single-word or single-digit substitutions still fail locally.
    let word_boxes = group_glyph_boxes(&raster.glyph_boxes);
    let mut worst_group_corr: f32 = 1.0;
    for group_box in &word_boxes {
        let Some(proj_box) = project_box_local(*group_box, params, bounds, resolution) else {
            continue;
        };
        let mut g_core = 0_usize;
        let mut group_corr = PearsonAccum::default();
        for ly in proj_box.top..proj_box.bottom {
            for lx in proj_box.left..proj_box.right {
                let idx = ly * bw + lx;
                if proj_glyph[idx] >= core_threshold {
                    g_core += 1;
                }
                group_corr.push(
                    f64::from(proj_glyph[idx]) / 255.0,
                    f64::from(fg_signal[idx]),
                );
            }
        }
        if g_core >= MIN_SEGMENT_CORE_PIXELS {
            let r = group_corr.correlation().unwrap_or(0.0) as f32;
            worst_group_corr = worst_group_corr.min(r);
        }
    }

    if scrim_total >= MIN_GLYPH_BOX_NON_GLYPH_PIXELS {
        let violation = scrim_violations as f32 / scrim_total as f32;
        if violation > config.max_extra_ink_ratio {
            return FrameCheckOutcome::Fail {
                recall,
                extra_ink: extra_ink_ratio.max(violation),
                detail: OverlayFindingKind::ScrimOrOutlineMismatch {
                    violation_percent: ratio_percent(violation),
                },
            };
        }
    }
    if outline_total >= MIN_GLYPH_BOX_NON_GLYPH_PIXELS {
        let violation = outline_violations as f32 / outline_total as f32;
        if violation > 0.30 {
            return FrameCheckOutcome::Fail {
                recall,
                extra_ink: extra_ink_ratio.max(violation),
                detail: OverlayFindingKind::ScrimOrOutlineMismatch {
                    violation_percent: ratio_percent(violation),
                },
            };
        }
    }

    if recall < config.min_glyph_match_ratio
        || global_corr < config.min_glyph_correlation
        || worst_group_corr < config.min_local_group_correlation
        || extra_ink_ratio > config.max_extra_ink_ratio
    {
        return FrameCheckOutcome::Fail {
            recall: recall
                .min(global_corr.max(0.0))
                .min(worst_group_corr.max(0.0)),
            extra_ink: extra_ink_ratio,
            detail: OverlayFindingKind::GlyphMismatch {
                recall_percent: ratio_percent(recall),
                worst_glyph_recall_percent: ratio_percent(worst_group_corr.max(0.0)),
                extra_ink_percent: ratio_percent(extra_ink_ratio),
            },
        };
    }

    FrameCheckOutcome::Pass {
        recall: recall
            .min(global_corr.max(0.0))
            .min(worst_group_corr.max(0.0)),
        extra_ink: extra_ink_ratio,
    }
}

#[allow(
    clippy::too_many_arguments,
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_possible_wrap
)]
fn check_partial_opacity_text_frame(
    proj_glyph: &[u8],
    proj_layer: &[u8],
    in_text_box: &[bool],
    in_scrim_inner: &[bool],
    near_glyph: &[bool],
    core_threshold: u8,
    fg_rgb: [u8; 3],
    scrim_active: bool,
    opacity: f32,
    bounds: TitlePixelBounds,
    resolution: (u32, u32),
    decoded_rgba8: &[u8],
    config: &OverlayVerificationConfig,
) -> FrameCheckOutcome {
    let bw = usize::try_from(bounds.right - bounds.left).unwrap_or(0);
    let bh = usize::try_from(bounds.bottom - bounds.top).unwrap_or(0);
    let fg_lin = fg_rgb.map(|c| decode_bt709(f32::from(c) / 255.0));
    let scrim_lin = [0x05_u8, 0x07, 0x0A].map(|c| decode_bt709(f32::from(c) / 255.0));
    let tol = config.luma_tolerance + 10.0;

    let mut total_core = 0_usize;
    let mut matched_core = 0_usize;
    let mut glyph_luma_sum = 0.0_f64;
    let mut non_glyph_luma_sum = 0.0_f64;
    let mut non_glyph_count = 0_usize;
    let mut scrim_total = 0_usize;
    let mut scrim_violations = 0_usize;

    for ly in 0..bh {
        let y = bounds.top + ly as i32;
        for lx in 0..bw {
            let x = bounds.left + lx as i32;
            let idx = ly * bw + lx;
            if proj_glyph[idx] >= core_threshold {
                total_core += 1;
                let alpha = (f32::from(proj_glyph[idx]) / 255.0) * opacity;
                let min_rgb = fg_lin.map(|c| encode_bt709(alpha * c) * 255.0 - tol);
                let max_rgb = fg_lin.map(|c| encode_bt709(alpha * c + (1.0 - alpha)) * 255.0 + tol);
                if neighborhood_matches_rgb_range(decoded_rgba8, resolution, x, y, min_rgb, max_rgb)
                {
                    matched_core += 1;
                }
                glyph_luma_sum +=
                    f64::from(rgb_to_ycbcr(decoded_rgb_at(decoded_rgba8, resolution, x, y))[0]);
            } else if !near_glyph[idx] && (in_scrim_inner[idx] || in_text_box[idx]) {
                let dec = decoded_rgb_at(decoded_rgba8, resolution, x, y);
                let y_val = rgb_to_ycbcr(dec)[0];
                non_glyph_luma_sum += f64::from(y_val);
                non_glyph_count += 1;
                if scrim_active && in_scrim_inner[idx] {
                    scrim_total += 1;
                    let a_scrim = (f32::from(proj_layer[idx]) / 255.0) * opacity;
                    let max_scrim = scrim_lin
                        .map(|c| encode_bt709(a_scrim * c + (1.0 - a_scrim)) * 255.0 + tol);
                    if (0..3).any(|c| f32::from(dec[c]) > max_scrim[c]) {
                        scrim_violations += 1;
                    }
                }
            }
        }
    }

    if total_core == 0 {
        return FrameCheckOutcome::Pass {
            recall: 1.0,
            extra_ink: 0.0,
        };
    }

    let match_ratio = matched_core as f32 / total_core as f32;
    if match_ratio < config.min_glyph_match_ratio {
        return FrameCheckOutcome::Fail {
            recall: match_ratio,
            extra_ink: 0.0,
            detail: OverlayFindingKind::FadeEnvelopeMismatch {
                match_percent: ratio_percent(match_ratio),
            },
        };
    }
    if scrim_total >= MIN_GLYPH_BOX_NON_GLYPH_PIXELS
        && (scrim_violations as f32 / scrim_total as f32) > config.max_extra_ink_ratio
    {
        return FrameCheckOutcome::Fail {
            recall: match_ratio,
            extra_ink: scrim_violations as f32 / scrim_total as f32,
            detail: OverlayFindingKind::ScrimOrOutlineMismatch {
                violation_percent: ratio_percent(scrim_violations as f32 / scrim_total as f32),
            },
        };
    }
    if opacity >= config.min_measurable_opacity
        && scrim_active
        && non_glyph_count >= MIN_GLYPH_BOX_NON_GLYPH_PIXELS
        && rgb_to_ycbcr(fg_rgb)[0] > 140.0
    {
        let mean_glyph = glyph_luma_sum / total_core as f64;
        let mean_scrim = non_glyph_luma_sum / non_glyph_count as f64;
        if mean_glyph <= mean_scrim + 8.0 {
            return FrameCheckOutcome::Fail {
                recall: 0.0,
                extra_ink: 1.0,
                detail: OverlayFindingKind::FadeEnvelopeMismatch { match_percent: 0 },
            };
        }
    }

    FrameCheckOutcome::Pass {
        recall: match_ratio,
        extra_ink: 0.0,
    }
}

#[allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_possible_wrap
)]
fn check_scrim_only_frame(
    proj_layer: &[u8],
    in_scrim_inner: &[bool],
    opacity: f32,
    bounds: TitlePixelBounds,
    resolution: (u32, u32),
    decoded_rgba8: &[u8],
    config: &OverlayVerificationConfig,
) -> FrameCheckOutcome {
    let bw = usize::try_from(bounds.right - bounds.left).unwrap_or(0);
    let bh = usize::try_from(bounds.bottom - bounds.top).unwrap_or(0);
    let scrim_lin = [0x05_u8, 0x07, 0x0A].map(|c| decode_bt709(f32::from(c) / 255.0));
    let tol = config.luma_tolerance + 10.0;
    let mut total = 0_usize;
    let mut violations = 0_usize;
    for ly in 0..bh {
        let y = bounds.top + ly as i32;
        for lx in 0..bw {
            let x = bounds.left + lx as i32;
            let idx = ly * bw + lx;
            if !in_scrim_inner[idx] || proj_layer[idx] < 64 {
                continue;
            }
            total += 1;
            let a = (f32::from(proj_layer[idx]) / 255.0) * opacity;
            let max_rgb = scrim_lin.map(|c| encode_bt709(a * c + (1.0 - a)) * 255.0 + tol);
            let dec = decoded_rgb_at(decoded_rgba8, resolution, x, y);
            if (0..3).any(|c| f32::from(dec[c]) > max_rgb[c]) {
                violations += 1;
            }
        }
    }
    if total >= MIN_GLYPH_BOX_NON_GLYPH_PIXELS
        && (violations as f32 / total as f32) > config.max_extra_ink_ratio
    {
        return FrameCheckOutcome::Fail {
            recall: 0.0,
            extra_ink: violations as f32 / total as f32,
            detail: OverlayFindingKind::ScrimOrOutlineMismatch {
                violation_percent: ratio_percent(violations as f32 / total as f32),
            },
        };
    }
    FrameCheckOutcome::Pass {
        recall: 1.0,
        extra_ink: 0.0,
    }
}

#[allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_possible_wrap
)]
fn check_card_overlay_on_frame(
    card_size: (u32, u32),
    card_rgba8: &[u8],
    params: &LayerParams,
    bounds: TitlePixelBounds,
    resolution: (u32, u32),
    decoded_rgba8: &[u8],
    config: &OverlayVerificationConfig,
) -> FrameCheckOutcome {
    let (width, height) = resolution;
    let extent = [width as f32, height as f32];
    let transformed = is_transformed(params);
    let search_radius: i32 = i32::from(transformed);
    let tol = config.luma_tolerance + 10.0;

    let mut total = 0_usize;
    let mut matched = 0_usize;

    for y in bounds.top..bounds.bottom {
        for x in bounds.left..bounds.right {
            let pixel = [x as usize, y as usize];
            if !quad_covers_pixel(params, extent, pixel)
                || !transition_covers_pixel(params, extent, pixel)
            {
                continue;
            }
            let ndc = [
                (x as f32 + 0.5) / extent[0] * 2.0 - 1.0,
                1.0 - (y as f32 + 0.5) / extent[1] * 2.0,
            ];
            let uv = quad_uv(params, ndc);
            if uv[0] < 0.0 || uv[0] > 1.0 || uv[1] < 0.0 || uv[1] > 1.0 {
                continue;
            }
            let expected = sample_rgba_bilinear(card_rgba8, card_size.0, card_size.1, uv);
            if expected[3] < 180 {
                continue;
            }
            total += 1;
            let expected_rgb = [expected[0], expected[1], expected[2]];
            if params.opacity >= 0.85 {
                let exp_ycbcr = rgb_to_ycbcr(expected_rgb);
                if pixel_matches_fg_in_neighborhood(
                    decoded_rgba8,
                    resolution,
                    x,
                    y,
                    search_radius,
                    exp_ycbcr,
                    config.luma_tolerance,
                    config.chroma_tolerance,
                ) {
                    matched += 1;
                }
            } else {
                let alpha = (f32::from(expected[3]) / 255.0) * params.opacity;
                let lin = expected_rgb.map(|c| decode_bt709(f32::from(c) / 255.0));
                let min_rgb = lin.map(|c| encode_bt709(alpha * c) * 255.0 - tol);
                let max_rgb = lin.map(|c| encode_bt709(alpha * c + (1.0 - alpha)) * 255.0 + tol);
                let dec = decoded_rgb_at(decoded_rgba8, resolution, x, y);
                if (0..3).all(|c| {
                    let v = f32::from(dec[c]);
                    v >= min_rgb[c] && v <= max_rgb[c]
                }) {
                    matched += 1;
                }
            }
        }
    }

    if total == 0 {
        return FrameCheckOutcome::Pass {
            recall: 1.0,
            extra_ink: 0.0,
        };
    }

    let ratio = matched as f32 / total as f32;
    if ratio < config.min_card_match_ratio {
        return FrameCheckOutcome::Fail {
            recall: ratio,
            extra_ink: 1.0 - ratio,
            detail: OverlayFindingKind::PinnedCardMismatch {
                match_percent: ratio_percent(ratio),
            },
        };
    }
    FrameCheckOutcome::Pass {
        recall: ratio,
        extra_ink: 0.0,
    }
}

struct LocalBox {
    left: usize,
    top: usize,
    right: usize,
    bottom: usize,
}

#[allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_possible_wrap
)]
fn project_box_local(
    box_bounds: TitlePixelBounds,
    params: &LayerParams,
    outer: TitlePixelBounds,
    resolution: (u32, u32),
) -> Option<LocalBox> {
    let width = f64::from(resolution.0);
    let height = f64::from(resolution.1);
    let anchor_x = f64::from(params.anchor_x) * width;
    let anchor_y = f64::from(params.anchor_y) * height;
    let offset_x = f64::from(params.offset_x) * width * 0.5;
    let offset_y = f64::from(params.offset_y) * height * 0.5;
    let (sin, cos) = f64::from(params.rotation).sin_cos();
    let mut left = f64::INFINITY;
    let mut right = f64::NEG_INFINITY;
    let mut top = f64::INFINITY;
    let mut bottom = f64::NEG_INFINITY;
    for x in [box_bounds.left, box_bounds.right] {
        for y in [box_bounds.top, box_bounds.bottom] {
            let dx = (f64::from(x) - anchor_x) * f64::from(params.scale_x);
            let dy = (f64::from(y) - anchor_y) * f64::from(params.scale_y);
            let px = anchor_x + dx * cos - dy * sin + offset_x;
            let py = anchor_y + dx * sin + dy * cos + offset_y;
            left = left.min(px);
            right = right.max(px);
            top = top.min(py);
            bottom = bottom.max(py);
        }
    }
    let l = (left.floor() as i32).max(outer.left) - outer.left;
    let t = (top.floor() as i32).max(outer.top) - outer.top;
    let r = (right.ceil() as i32).min(outer.right) - outer.left;
    let b = (bottom.ceil() as i32).min(outer.bottom) - outer.top;
    if l >= r || t >= b {
        return None;
    }
    Some(LocalBox {
        left: usize::try_from(l).ok()?,
        top: usize::try_from(t).ok()?,
        right: usize::try_from(r).ok()?,
        bottom: usize::try_from(b).ok()?,
    })
}

#[allow(clippy::float_cmp)]
pub(crate) fn is_transformed(params: &LayerParams) -> bool {
    params.scale_x != 1.0
        || params.scale_y != 1.0
        || params.rotation != 0.0
        || params.offset_x != 0.0
        || params.offset_y != 0.0
}

#[allow(clippy::float_cmp)]
pub(crate) fn quad_uv(params: &LayerParams, ndc: [f32; 2]) -> [f32; 2] {
    let anchor = [params.anchor_x * 2.0 - 1.0, 1.0 - params.anchor_y * 2.0];
    let mut rel = [
        ndc[0] - anchor[0] - params.offset_x,
        ndc[1] - anchor[1] + params.offset_y,
    ];
    if params.rotation != 0.0 {
        let (sin_r, cos_r) = params.rotation.sin_cos();
        let rot = [rel[0], rel[1] * params.frame_aspect];
        rel = [
            rot[0] * cos_r - rot[1] * sin_r,
            (rot[0] * sin_r + rot[1] * cos_r) / params.frame_aspect,
        ];
    }
    let corner = [
        rel[0] / params.scale_x + anchor[0],
        rel[1] / params.scale_y + anchor[1],
    ];
    [f32::midpoint(corner[0], 1.0), (1.0 - corner[1]) * 0.5]
}

fn neighborhood_matches_rgb_range(
    decoded_rgba8: &[u8],
    resolution: (u32, u32),
    x: i32,
    y: i32,
    min_rgb: [f32; 3],
    max_rgb: [f32; 3],
) -> bool {
    for dy in -1..=1 {
        for dx in -1..=1 {
            let dec = decoded_rgb_at(decoded_rgba8, resolution, x + dx, y + dy);
            if (0..3).all(|c| {
                let v = f32::from(dec[c]);
                v >= min_rgb[c] && v <= max_rgb[c]
            }) {
                return true;
            }
        }
    }
    false
}

#[allow(clippy::cast_precision_loss, clippy::float_cmp)]
pub(crate) fn quad_covers_pixel(p: &LayerParams, size: [f32; 2], pixel: [usize; 2]) -> bool {
    let anchor = [p.anchor_x * 2.0 - 1.0, 1.0 - p.anchor_y * 2.0];
    let corner = |x: f32, y: f32| {
        let c = [(x - anchor[0]) * p.scale_x, (y - anchor[1]) * p.scale_y];
        let r = if p.rotation == 0.0 {
            c
        } else {
            let (s, cos, a) = (p.rotation.sin(), p.rotation.cos(), p.frame_aspect);
            [c[0] * cos + c[1] * a * s, (-c[0] * s + c[1] * a * cos) / a]
        };
        let t = [r[0] + anchor[0] + p.offset_x, r[1] + anchor[1] - p.offset_y];
        let snap = |v: f32| (f64::from(v) * 256.0).round() / 256.0;
        [
            snap(f32::midpoint(t[0], 1.0) * size[0]),
            snap(((1.0 - t[1]) * 0.5) * size[1]),
        ]
    };
    let mut quad = [
        corner(-1.0, -1.0),
        corner(1.0, -1.0),
        corner(1.0, 1.0),
        corner(-1.0, 1.0),
    ];
    let edges = |q: [[f64; 2]; 4]| (0..4).map(move |k| (q[k], q[(k + 1) % 4]));
    let area: f64 = edges(quad).map(|(a, b)| a[0] * b[1] - b[0] * a[1]).sum();
    if area < 0.0 {
        quad.reverse();
    }
    let pt = [pixel[0] as f64 + 0.5, pixel[1] as f64 + 0.5];
    area != 0.0
        && edges(quad).all(|(a, b)| {
            let value = (b[0] - a[0]) * (pt[1] - a[1]) - (b[1] - a[1]) * (pt[0] - a[0]);
            let top_left = (b[1] == a[1] && b[0] > a[0]) || b[1] < a[1];
            value > 0.0 || (value == 0.0 && top_left)
        })
}

#[allow(clippy::cast_precision_loss)]
pub(crate) fn transition_covers_pixel(
    p: &LayerParams,
    extent: [f32; 2],
    pixel: [usize; 2],
) -> bool {
    if p.coverage_on <= 0.5 {
        return true;
    }
    let axis = usize::from(p.coverage_axis > 0.5);
    let centre = pixel[axis] as f64 + 0.5;
    let below = centre < f64::from(p.coverage_edge) * f64::from(extent[axis]);
    below == (p.coverage_on < 1.5)
}

#[allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss
)]
fn sample_mask_bilinear(mask: &[u8], width: u32, height: u32, uv: [f32; 2]) -> u8 {
    let x = uv[0] * width as f32 - 0.5;
    let y = uv[1] * height as f32 - 0.5;
    let x0 = x.floor() as i64;
    let y0 = y.floor() as i64;
    let fx = x - x0 as f32;
    let fy = y - y0 as f32;
    let at = |cx: i64, cy: i64| -> f32 {
        let cx = cx.clamp(0, i64::from(width) - 1) as usize;
        let cy = cy.clamp(0, i64::from(height) - 1) as usize;
        f32::from(mask[cy * width as usize + cx])
    };
    let top = at(x0, y0) * (1.0 - fx) + at(x0 + 1, y0) * fx;
    let bot = at(x0, y0 + 1) * (1.0 - fx) + at(x0 + 1, y0 + 1) * fx;
    (top * (1.0 - fy) + bot * fy).round().clamp(0.0, 255.0) as u8
}

#[allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss
)]
pub(crate) fn sample_rgba_bilinear(rgba: &[u8], width: u32, height: u32, uv: [f32; 2]) -> [u8; 4] {
    let x = uv[0] * width as f32 - 0.5;
    let y = uv[1] * height as f32 - 0.5;
    let x0 = x.floor() as i64;
    let y0 = y.floor() as i64;
    let fx = x - x0 as f32;
    let fy = y - y0 as f32;
    let at = |cx: i64, cy: i64, c: usize| -> f32 {
        let cx = cx.clamp(0, i64::from(width) - 1) as usize;
        let cy = cy.clamp(0, i64::from(height) - 1) as usize;
        f32::from(rgba[(cy * width as usize + cx) * 4 + c])
    };
    std::array::from_fn(|c| {
        let top = at(x0, y0, c) * (1.0 - fx) + at(x0 + 1, y0, c) * fx;
        let bot = at(x0, y0 + 1, c) * (1.0 - fx) + at(x0 + 1, y0 + 1, c) * fx;
        (top * (1.0 - fy) + bot * fy).round().clamp(0.0, 255.0) as u8
    })
}

#[allow(clippy::cast_possible_wrap, clippy::cast_sign_loss)]
fn decoded_rgb_at(rgba8: &[u8], resolution: (u32, u32), x: i32, y: i32) -> [u8; 3] {
    let cx = x.clamp(0, resolution.0 as i32 - 1) as usize;
    let cy = y.clamp(0, resolution.1 as i32 - 1) as usize;
    let offset = (cy * resolution.0 as usize + cx) * 4;
    [rgba8[offset], rgba8[offset + 1], rgba8[offset + 2]]
}

fn rgb_to_ycbcr(rgb: [u8; 3]) -> [f32; 3] {
    let r = f32::from(rgb[0]);
    let g = f32::from(rgb[1]);
    let b = f32::from(rgb[2]);
    let y = 0.2126 * r + 0.7152 * g + 0.0722 * b;
    let cb = (b - y) / 1.8556;
    let cr = (r - y) / 1.5748;
    [y, cb, cr]
}

fn ycbcr_matches(actual: [f32; 3], expected: [f32; 3], luma_tol: f32, chroma_tol: f32) -> bool {
    (actual[0] - expected[0]).abs() <= luma_tol
        && (actual[1] - expected[1]).abs() <= chroma_tol
        && (actual[2] - expected[2]).abs() <= chroma_tol
}

#[derive(Default)]
struct PearsonAccum {
    n: f64,
    sum_x: f64,
    sum_y: f64,
    sum_xx: f64,
    sum_yy: f64,
    sum_xy: f64,
}

impl PearsonAccum {
    fn push(&mut self, x: f64, y: f64) {
        self.n += 1.0;
        self.sum_x += x;
        self.sum_y += y;
        self.sum_xx += x * x;
        self.sum_yy += y * y;
        self.sum_xy += x * y;
    }

    fn correlation(&self) -> Option<f64> {
        if self.n < 4.0 {
            return None;
        }
        let num = self.n * self.sum_xy - self.sum_x * self.sum_y;
        let dx = self.n * self.sum_xx - self.sum_x * self.sum_x;
        let dy = self.n * self.sum_yy - self.sum_y * self.sum_y;
        if dx <= 1e-9 || dy <= 1e-9 {
            return None;
        }
        Some(num / (dx * dy).sqrt())
    }
}

/// Group adjacent glyph boxes on the same text line into word-scale boxes (padded by 1px) so
/// localized correlation works reliably at low resolutions while still isolating single words.
fn group_glyph_boxes(glyphs: &[TitlePixelBounds]) -> Vec<TitlePixelBounds> {
    let mut groups: Vec<TitlePixelBounds> = Vec::new();
    for &g in glyphs {
        if let Some(last) = groups.last_mut() {
            let last_h = (last.bottom - last.top).max(1);
            let same_line = g.top < last.bottom + 2 && g.bottom > last.top - 2;
            let gap = g.left - last.right;
            let close_x = gap >= -2 && gap <= (last_h / 2).max(3);
            let merged_w = g.right.max(last.right) - last.left.min(g.left);
            if same_line && close_x && merged_w <= last_h * 6 {
                last.left = last.left.min(g.left);
                last.top = last.top.min(g.top);
                last.right = last.right.max(g.right);
                last.bottom = last.bottom.max(g.bottom);
                continue;
            }
        }
        groups.push(g);
    }
    for group in &mut groups {
        group.left = group.left.saturating_sub(1);
        group.top = group.top.saturating_sub(1);
        group.right = group.right.saturating_add(1);
        group.bottom = group.bottom.saturating_add(1);
    }
    groups
}

#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn expected_composited_rgb(layer_rgba: [u8; 4], bg_lin: [f32; 3], opacity: f32) -> [u8; 3] {
    let alpha = ((f32::from(layer_rgba[3]) / 255.0) * opacity).clamp(0.0, 1.0);
    std::array::from_fn(|c| {
        let src = decode_bt709(f32::from(layer_rgba[c]) / 255.0);
        let lin = src * alpha + bg_lin[c] * (1.0 - alpha);
        (encode_bt709(lin.clamp(0.0, 1.0)) * 255.0)
            .round()
            .clamp(0.0, 255.0) as u8
    })
}

#[allow(clippy::too_many_arguments)]
fn pixel_matches_either_in_neighborhood(
    rgba8: &[u8],
    resolution: (u32, u32),
    x: i32,
    y: i32,
    radius: i32,
    first_ycbcr: [f32; 3],
    second_ycbcr: [f32; 3],
    luma_tol: f32,
    chroma_tol: f32,
) -> bool {
    for dy in -radius..=radius {
        for dx in -radius..=radius {
            let dec = rgb_to_ycbcr(decoded_rgb_at(rgba8, resolution, x + dx, y + dy));
            if ycbcr_matches(dec, first_ycbcr, luma_tol, chroma_tol)
                || ycbcr_matches(dec, second_ycbcr, luma_tol, chroma_tol)
            {
                return true;
            }
        }
    }
    false
}

#[allow(clippy::too_many_arguments)]
fn pixel_matches_fg_in_neighborhood(
    rgba8: &[u8],
    resolution: (u32, u32),
    x: i32,
    y: i32,
    radius: i32,
    expected_ycbcr: [f32; 3],
    luma_tol: f32,
    chroma_tol: f32,
) -> bool {
    for dy in -radius..=radius {
        for dx in -radius..=radius {
            let dec = decoded_rgb_at(rgba8, resolution, x + dx, y + dy);
            if ycbcr_matches(rgb_to_ycbcr(dec), expected_ycbcr, luma_tol, chroma_tol) {
                return true;
            }
        }
    }
    false
}

#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn ratio_percent(ratio: f32) -> u8 {
    (ratio.clamp(0.0, 1.0) * 100.0).round() as u8
}

/// Decode a single verified still image (PNG/JPEG/etc.) from a verified stream into RGBA8.
pub(crate) fn decode_still_image_rgba8(
    reader: Box<dyn ReviewReadSeek>,
    cancellation: &ExportCancellation,
) -> Result<(u32, u32, Vec<u8>), MediaError> {
    crate::quiet_ffmpeg_log();
    let io = ffmpeg::format::context::StreamIo::from_read_seek(reader).map_err(backend)?;
    let cancel = cancellation.clone();
    let mut input =
        ffmpeg::format::input_from_stream_with_interrupt(io, Some("card"), None, move || {
            cancel.is_cancelled()
        })
        .map_err(backend)?;
    let stream = input
        .streams()
        .best(ffmpeg::media::Type::Video)
        .ok_or_else(|| MediaError::Backend("pinned card image has no video stream".into()))?;
    let stream_index = stream.index();
    let time_base = stream.time_base();
    let mut decoder = ffmpeg::codec::context::Context::from_parameters(stream.parameters())
        .and_then(|context| context.decoder().video())
        .map_err(backend)?;

    let mut frame = ffmpeg::frame::Video::empty();
    let mut got_frame = false;
    loop {
        if cancellation.is_cancelled() {
            return Err(MediaError::Cancelled);
        }
        let mut packet = ffmpeg::Packet::empty();
        match packet.read(&mut input) {
            Ok(()) if packet.stream() == stream_index => {
                decoder.send_packet(&packet).map_err(backend)?;
                match decoder.receive_frame(&mut frame) {
                    Ok(()) => {
                        got_frame = true;
                        break;
                    }
                    Err(ffmpeg::Error::Other { errno }) if errno == ffmpeg::error::EAGAIN => {}
                    Err(error) => return Err(backend(error)),
                }
            }
            Ok(()) => {}
            Err(ffmpeg::Error::Eof) => break,
            Err(error) => return Err(backend(error)),
        }
    }
    if !got_frame {
        decoder.send_eof().map_err(backend)?;
        decoder.receive_frame(&mut frame).map_err(backend)?;
    }

    let width = frame.width();
    let height = frame.height();
    if width == 0 || height == 0 {
        return Err(MediaError::Backend(
            "pinned card image decoded to zero dimensions".into(),
        ));
    }
    let mut description = color_description_from_decoder(&decoder, Some(frame.format()));
    let is_rgb = is_rgb_pixel(frame.format());
    if is_rgb {
        description.matrix = ColorMatrix::Rgb;
        description.range = ColorRange::Full;
    }
    let mut graph = managed_filter_graph(
        Path::new("card"),
        &decoder,
        time_base,
        width,
        height,
        &description,
    )?;
    let mut rgba8 = Vec::new();
    let mut filter_pts = 0_i64;
    scale_frame_to_rgba8(&mut graph, frame, &mut filter_pts, is_rgb, &mut rgba8)?;
    Ok((width, height, rgba8))
}

fn is_rgb_pixel(pixel: ffmpeg::format::Pixel) -> bool {
    pixel
        .descriptor()
        .is_some_and(|desc| desc.name().contains("rgb") || desc.name().contains("bgr"))
}

pub(crate) struct DecodedReviewVideoFrame<'a> {
    pub(crate) frame_index: u64,
    pub(crate) pts: i64,
    pub(crate) dimensions: (u32, u32),
    pub(crate) rgba8: &'a [u8],
}

/// Bounded streaming decoder over a verified review artifact [`File`].
/// Keeps at most one converted RGBA8 frame resident in memory.
pub(crate) struct ReviewArtifactVideoDecoder {
    input: ffmpeg::format::context::Input,
    decoder: ffmpeg::decoder::Video,
    stream_index: usize,
    stream_time_base: ffmpeg::Rational,
    target_size: (u32, u32),
    graph: Option<(ffmpeg::filter::Graph, bool)>,
    filter_pts: i64,
    next_index: u64,
    eof_sent: bool,
    eof_reached: bool,
    rgba8: Vec<u8>,
}

impl ReviewArtifactVideoDecoder {
    pub(crate) fn open(
        file: File,
        timing: &ReviewOutputTiming,
        target_size: (u32, u32),
        cancellation: &ExportCancellation,
    ) -> Result<Self, MediaError> {
        if cancellation.is_cancelled() {
            return Err(MediaError::Cancelled);
        }
        crate::quiet_ffmpeg_log();
        if ffmpeg::codec::version() != timing.decoder_version {
            return Err(MediaError::Backend(format!(
                "review decoder version {} does not match recorded {}",
                ffmpeg::codec::version(),
                timing.decoder_version
            )));
        }
        let io = ffmpeg::format::context::StreamIo::from_read_seek(file).map_err(backend)?;
        let cancel = cancellation.clone();
        let input = ffmpeg::format::input_from_stream_with_interrupt(
            io,
            Some("review.mp4"),
            None,
            move || cancel.is_cancelled(),
        )
        .map_err(backend)?;
        let stream = input
            .streams()
            .find(|s| s.index() == timing.video.index)
            .ok_or_else(|| {
                MediaError::Backend("recorded review video stream index missing".into())
            })?;
        let time_base = stream.time_base();
        if (time_base.numerator(), time_base.denominator()) != timing.video.time_base {
            return Err(MediaError::Backend(
                "review video stream time base changed".into(),
            ));
        }
        let decoder = ffmpeg::codec::context::Context::from_parameters(stream.parameters())
            .and_then(|context| context.decoder().video())
            .map_err(backend)?;
        let codec_name = decoder
            .codec()
            .map(|c| c.name().to_owned())
            .ok_or_else(|| MediaError::Backend("review video decoder unavailable".into()))?;
        if codec_name != timing.video.decoder {
            return Err(MediaError::Backend(format!(
                "review video decoder {codec_name} does not match recorded {}",
                timing.video.decoder
            )));
        }
        Ok(Self {
            input,
            decoder,
            stream_index: timing.video.index,
            stream_time_base: time_base,
            target_size,
            graph: None,
            filter_pts: 0,
            next_index: 0,
            eof_sent: false,
            eof_reached: false,
            rgba8: Vec::new(),
        })
    }

    pub(crate) const fn next_frame_index(&self) -> u64 {
        self.next_index
    }

    pub(crate) fn next_frame(
        &mut self,
        convert_rgba8: bool,
        cancellation: &ExportCancellation,
    ) -> Result<Option<DecodedReviewVideoFrame<'_>>, MediaError> {
        if self.eof_reached {
            return Ok(None);
        }
        let mut decoded = ffmpeg::frame::Video::empty();
        loop {
            if cancellation.is_cancelled() {
                return Err(MediaError::Cancelled);
            }
            match self.decoder.receive_frame(&mut decoded) {
                Ok(()) => {
                    let pts = decoded
                        .pts()
                        .or_else(|| decoded.timestamp())
                        .ok_or_else(|| {
                            MediaError::Backend("decoded review frame missing PTS".into())
                        })?;
                    let dimensions = (decoded.width(), decoded.height());
                    let frame_index = self.next_index;
                    self.next_index = self.next_index.saturating_add(1);
                    if convert_rgba8 {
                        if self.graph.is_none() {
                            let mut description = color_description_from_decoder(
                                &self.decoder,
                                Some(decoded.format()),
                            );
                            let is_rgb = is_rgb_pixel(decoded.format());
                            if is_rgb {
                                description.matrix = ColorMatrix::Rgb;
                                description.range = ColorRange::Full;
                            } else if matches!(description.range, ColorRange::Unknown) {
                                description.range = ColorRange::Limited;
                            }
                            let graph = managed_filter_graph(
                                Path::new("review.mp4"),
                                &self.decoder,
                                self.stream_time_base,
                                self.target_size.0,
                                self.target_size.1,
                                &description,
                            )?;
                            self.graph = Some((graph, is_rgb));
                        }
                        let (graph, is_rgb) = self.graph.as_mut().expect("initialized graph");
                        scale_frame_to_rgba8(
                            graph,
                            decoded,
                            &mut self.filter_pts,
                            *is_rgb,
                            &mut self.rgba8,
                        )?;
                    }
                    return Ok(Some(DecodedReviewVideoFrame {
                        frame_index,
                        pts,
                        dimensions,
                        rgba8: &self.rgba8,
                    }));
                }
                Err(ffmpeg::Error::Eof) => {
                    self.eof_reached = true;
                    return Ok(None);
                }
                Err(ffmpeg::Error::Other { errno }) if errno == ffmpeg::error::EAGAIN => {
                    if self.eof_sent {
                        self.eof_reached = true;
                        return Ok(None);
                    }
                    let mut packet = ffmpeg::Packet::empty();
                    match packet.read(&mut self.input) {
                        Ok(()) if packet.stream() == self.stream_index => {
                            self.decoder.send_packet(&packet).map_err(backend)?;
                        }
                        Ok(()) => {}
                        Err(ffmpeg::Error::Eof) => {
                            self.decoder.send_eof().map_err(backend)?;
                            self.eof_sent = true;
                        }
                        Err(error) => return Err(backend(error)),
                    }
                }
                Err(error) => return Err(backend(error)),
            }
        }
    }
}

#[allow(clippy::cast_possible_truncation)]
fn scale_frame_to_rgba8(
    graph: &mut ffmpeg::filter::Graph,
    mut frame: ffmpeg::frame::Video,
    filter_pts: &mut i64,
    full_u16_range: bool,
    out_rgba8: &mut Vec<u8>,
) -> Result<(), MediaError> {
    *filter_pts = filter_pts.saturating_add(1);
    frame.set_pts(Some(*filter_pts));
    {
        let mut source = graph
            .get("source")
            .ok_or_else(|| MediaError::Backend("review filter source disappeared".into()))?;
        source.source().add(&frame).map_err(backend)?;
    }
    let mut converted = ffmpeg::frame::Video::empty();
    {
        let mut sink = graph
            .get("sink")
            .ok_or_else(|| MediaError::Backend("review filter sink disappeared".into()))?;
        sink.sink().frame(&mut converted).map_err(backend)?;
    }
    let width = converted.width() as usize;
    let height = converted.height() as usize;
    let stride = converted.stride(0);
    let data = converted.data(0);
    out_rgba8.clear();
    out_rgba8.reserve(width.saturating_mul(height).saturating_mul(4));
    for row in 0..height {
        let start = row.saturating_mul(stride);
        let end = start.saturating_add(width.saturating_mul(8));
        for sample in data[start..end].as_chunks::<2>().0 {
            let v = u32::from(u16::from_le_bytes([sample[0], sample[1]]));
            let byte = if full_u16_range {
                ((v * 255 + 32_767) / 65_535).min(255) as u8
            } else {
                ((v + 128) >> 8).min(255) as u8
            };
            out_rgba8.push(byte);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;
