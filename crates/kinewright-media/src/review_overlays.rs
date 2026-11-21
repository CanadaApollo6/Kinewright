//! Pure decoded-region verification of captions, titles and pinned cards against frozen review
//! expectations using the export compositor's own with/without-overlay references (Ruling L14).
//!
//! Reads only the verified artifact stream, frozen expectations, and frozen `PlaybackInputs`
//! (never ambient filesystem paths or the working edit). Renders and compares one frame at a
//! time with O(1) memory independent of overlay count.

use std::{
    cmp::Reverse,
    collections::{BTreeMap, BTreeSet, BinaryHeap},
    fs::File,
    ops::Range,
    path::Path,
    sync::Arc,
};

use ffmpeg_next as ffmpeg;
use kinewright_core::{
    Clip, ClipContent, ClipId, ColorMatrix, ColorRange, Document, ExactMicros, ExportCancellation,
    FrameRounding, MediaError, Rational, TimeCode, TitlePixelBounds, TrackKind,
    map_frames_with_rounding,
    output_review::{
        OverlayKind, OverlayPixels, OverlayText, ReviewExpectations, ReviewOverlay, ReviewRowResult,
    },
};
use serde::{Deserialize, Serialize};

use crate::{
    compositor::{GpuContext, LayerParams},
    decode::{backend, color_description_from_decoder, managed_filter_graph},
    locator::DenyLocatorOpens,
    owned_io::PlaybackInputs,
    render::{DecodeStrategy, FrameRenderer, RenderScale},
    review_output::ReviewOutputTiming,
};

/// Cap on findings retained in one [`OverlayVerificationReport`].
pub const MAX_OVERLAY_FINDINGS: usize = 64;
/// Maximum output frame index scanned during overlay verification.
pub const MAX_OVERLAY_FRAME_SCAN: u64 = 1_000_000;
/// Bounded decode window: one decoded video frame resident at a time.
pub const OVERLAY_DECODE_WINDOW_FRAMES: usize = 1;

const TILE_SIZE: usize = 8;
const TILE_STEP: usize = 4;
const TILE_MIN_SUPPORT_PIXELS: usize = 8;
const BOX_PAD_PX: i32 = 2;

/// Fixed, validated configuration governing L14 with/without-overlay verification.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OverlayVerificationConfig {
    /// Minimum BT.709 YCbCr distance (0..=255 scale) between with-overlay and without-overlay
    /// compositor references for a pixel to join the overlay's active support set.
    pub min_pixel_signal: f32,
    /// Minimum number of active support pixels on a frame for that frame to be measurable.
    pub min_support_pixels: usize,
    /// Minimum RMS reference separation (`d(with, without)`) over active support pixels for a
    /// frame to be measurable. Frames below this floor are `NotMeasurable`, never silent passes.
    pub min_reference_separation: f32,
    /// Maximum allowed `with_rms / without_rms` ratio on measurable frames (the decoded output
    /// must match the with-overlay reference clearly better than the without-overlay reference).
    pub max_with_to_without_ratio: f32,
    /// Minimum absolute advantage `without_rms - with_rms` on measurable frames.
    pub min_advantage_margin: f32,
    /// Absolute RMS ceiling on `d(decoded, with_overlay)` over active support pixels.
    pub max_codec_error_rms: f32,
    /// Relative RMS ceiling (`ref_rms * max_relative_error`) on `d(decoded, with_overlay)`.
    pub max_relative_error: f32,
    /// Localized `8x8` overlapping tile RMS ceiling on `d(decoded, with_overlay)` inside the
    /// overlay bounds (catches single-digit or single-word substitutions in long lines).
    pub max_tile_error_rms: f32,
    /// Minimum fraction of an element's expected visible frames that must be measurable for the
    /// element to pass (in addition to at least one measurable frame per animated extreme).
    pub min_measurable_frame_ratio: f32,
    /// Maximum allowed RMS departure from `without_overlay` on outside-cue timing probes
    /// (`cue_start - 1` and `cue_end`) before flagging an early or lingering overlay.
    pub timing_max_without_rms: f32,
    /// Maximum number of findings recorded in the report (must be in `1..=MAX_OVERLAY_FINDINGS`).
    pub max_findings: usize,
}

impl Default for OverlayVerificationConfig {
    fn default() -> Self {
        Self {
            min_pixel_signal: 6.0,
            min_support_pixels: 16,
            min_reference_separation: 12.0,
            max_with_to_without_ratio: 0.45,
            min_advantage_margin: 6.0,
            max_codec_error_rms: 24.0,
            max_relative_error: 0.30,
            max_tile_error_rms: 26.0,
            min_measurable_frame_ratio: 0.50,
            timing_max_without_rms: 22.0,
            max_findings: MAX_OVERLAY_FINDINGS,
        }
    }
}

impl OverlayVerificationConfig {
    /// Validate that every threshold is finite and within its permitted domain.
    ///
    /// # Errors
    /// Returns [`MediaError::Backend`] when any threshold is `NaN`, infinite, or out of range.
    pub fn validate(&self) -> Result<(), MediaError> {
        let check_range = |name: &str, v: f32, lo: f32, hi: f32| -> Result<(), MediaError> {
            if !v.is_finite() || v < lo || v > hi {
                return Err(MediaError::Backend(format!(
                    "invalid overlay verification config: {name}={v} must be finite in {lo}..={hi}"
                )));
            }
            Ok(())
        };
        check_range("min_pixel_signal", self.min_pixel_signal, 1.0, 64.0)?;
        check_range(
            "min_reference_separation",
            self.min_reference_separation,
            2.0,
            128.0,
        )?;
        if self.min_reference_separation < self.min_pixel_signal {
            return Err(MediaError::Backend(
                "invalid overlay verification config: min_reference_separation must be >= min_pixel_signal"
                    .into(),
            ));
        }
        check_range(
            "max_with_to_without_ratio",
            self.max_with_to_without_ratio,
            0.05,
            0.90,
        )?;
        check_range("min_advantage_margin", self.min_advantage_margin, 1.0, 64.0)?;
        check_range("max_codec_error_rms", self.max_codec_error_rms, 2.0, 64.0)?;
        check_range("max_relative_error", self.max_relative_error, 0.05, 0.75)?;
        check_range("max_tile_error_rms", self.max_tile_error_rms, 4.0, 80.0)?;
        check_range(
            "min_measurable_frame_ratio",
            self.min_measurable_frame_ratio,
            0.05,
            1.0,
        )?;
        check_range(
            "timing_max_without_rms",
            self.timing_max_without_rms,
            2.0,
            64.0,
        )?;
        if self.min_support_pixels == 0 || self.min_support_pixels > 10_000 {
            return Err(MediaError::Backend(format!(
                "invalid overlay verification config: min_support_pixels={} must be in 1..=10000",
                self.min_support_pixels
            )));
        }
        if self.max_findings == 0 || self.max_findings > MAX_OVERLAY_FINDINGS {
            return Err(MediaError::Backend(format!(
                "invalid overlay verification config: max_findings={} must be in 1..={MAX_OVERLAY_FINDINGS}",
                self.max_findings
            )));
        }
        Ok(())
    }

    /// Stable canonical SHA-256 digest of this overlay verification configuration, ready to be
    /// combined with other review-check digests into one `ReviewCheckBinding`.
    #[must_use]
    pub fn sha256(&self) -> [u8; 32] {
        let mut hasher = kinewright_hash::Sha256::new();
        hasher.update(b"kinewright-overlay-verification-config-v2\0");
        for value in [
            self.min_pixel_signal,
            self.min_reference_separation,
            self.max_with_to_without_ratio,
            self.min_advantage_margin,
            self.max_codec_error_rms,
            self.max_relative_error,
            self.max_tile_error_rms,
            self.min_measurable_frame_ratio,
            self.timing_max_without_rms,
        ] {
            let normalized = if value == 0.0 { 0.0_f32 } else { value };
            hasher.update(&normalized.to_bits().to_le_bytes());
        }
        hasher.update(&(u64::try_from(self.min_support_pixels).unwrap_or(u64::MAX)).to_le_bytes());
        hasher.update(&(u64::try_from(self.max_findings).unwrap_or(u64::MAX)).to_le_bytes());
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
    pub measurable_frames: u64,
    pub unmeasurable_frames: u64,
    /// Number of frames actually measured (equals `measurable_frames`; unmeasurable frames are
    /// never counted as checked).
    pub checked_frames: u64,
    pub animated_extreme_frames: u64,
    pub measured_extreme_frames: u64,
    pub boundary_checked_frames: u64,
    pub failed_frames: u64,
    pub worst_with_rms: f32,
    pub min_without_rms: f32,
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
    UnmeasurableOverlay {
        expected_frames: u64,
        measurable_frames: u64,
        expected_extremes: u64,
        measured_extremes: u64,
    },
    CompositorReferenceMismatch {
        with_rms_tenths: u16,
        without_rms_tenths: u16,
        ref_rms_tenths: u16,
        worst_tile_rms_tenths: u16,
    },
    UnexpectedOverlayOutsideCue {
        without_rms_tenths: u16,
        with_injected_rms_tenths: u16,
        ref_rms_tenths: u16,
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
    pub unmeasurable_frames: u64,
    pub animated_extreme_frames: u64,
    pub boundary_checked_frames: u64,
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
                unmeasurable_frames: 0,
                animated_extreme_frames: 0,
                boundary_checked_frames: 0,
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

/// Shared decoded card pixel buffer (stored behind `Arc` to prevent per-instance cloning).
pub(crate) type DecodedCardCache = BTreeMap<([u8; 32], u64), (u32, u32, Arc<Vec<u8>>)>;

/// Verify every expected caption, title, and pinned card on every frame of its visible range
/// (plus outside-cue timing boundaries) by rendering the frozen document through the export's
/// own compositor with and without each overlay (Ruling L14).
///
/// # Errors
/// Returns [`MediaError::Cancelled`] when `cancellation` fires or [`MediaError::Backend`] when
/// `config` fails validation. Stream, timing, or expectation gaps that prevent deciding are
/// reported in the returned report as [`ReviewRowResult::Unknown`] (never [`ReviewRowResult::Pass`]).
#[allow(clippy::too_many_lines)]
pub fn verify_review_overlays(
    artifact_file: File,
    artifact_timing: &ReviewOutputTiming,
    expectations: &ReviewExpectations,
    inputs: &Arc<PlaybackInputs>,
    gpu: GpuContext,
    config: &OverlayVerificationConfig,
    cancellation: &ExportCancellation,
) -> Result<OverlayVerificationReport, MediaError> {
    if cancellation.is_cancelled() {
        return Err(MediaError::Cancelled);
    }
    config.validate()?;
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
                    reason: "invalid output raster, frame rate, or duration".into(),
                },
            },
        );
        return Ok(report);
    }

    let total_output_frames = u64::try_from(expectations.output_frames.0).unwrap_or(0);
    if total_output_frames > MAX_OVERLAY_FRAME_SCAN {
        report.push_finding(
            finding_cap,
            OverlayFinding {
                element: None,
                overlay_kind: None,
                frame_index: None,
                pts: None,
                detail: OverlayFindingKind::UnresolvedExpectation {
                    reason: format!(
                        "output duration {total_output_frames} exceeds overlay scan cap {MAX_OVERLAY_FRAME_SCAN}"
                    ),
                },
            },
        );
        return Ok(report);
    }

    // Opus m6 / L12: unknown or non-positive frame step prevents verifying dropped/duplicated frames.
    if !artifact_timing.video_pts_step.is_some_and(|step| step > 0) {
        report.push_finding(
            finding_cap,
            OverlayFinding {
                element: None,
                overlay_kind: None,
                frame_index: None,
                pts: None,
                detail: OverlayFindingKind::StreamUnreadable {
                    reason: "recorded video frame PTS step is unknown or non-positive".into(),
                },
            },
        );
        return Ok(report);
    }

    let frozen_document = inputs.document();
    let mut unresolved = false;
    for (label, inventory) in [
        ("captions", &expectations.overlays.captions),
        ("titles", &expectations.overlays.titles),
        ("cards", &expectations.overlays.cards),
    ] {
        if !inventory.is_determined() {
            unresolved = true;
            report.push_finding(
                finding_cap,
                OverlayFinding {
                    element: None,
                    overlay_kind: None,
                    frame_index: None,
                    pts: None,
                    detail: OverlayFindingKind::UnresolvedExpectation {
                        reason: format!("{label} inventory is undetermined"),
                    },
                },
            );
        }
    }

    let all_overlays: Vec<&ReviewOverlay> = expectations
        .overlays
        .captions
        .known()
        .chain(expectations.overlays.titles.known())
        .chain(expectations.overlays.cards.known())
        .collect();

    // Prepare lightweight per-overlay schedules ONLY (zero per-element raster buffers: Opus B1).
    let mut schedules: Vec<OverlaySchedule> = Vec::with_capacity(all_overlays.len());
    for overlay in all_overlays {
        match build_overlay_schedule(overlay, expectations, frozen_document, total_output_frames) {
            Ok(schedule) => schedules.push(schedule),
            Err(reason) => {
                unresolved = true;
                report.push_finding(
                    finding_cap,
                    OverlayFinding {
                        element: Some(overlay.element),
                        overlay_kind: Some(overlay.kind),
                        frame_index: None,
                        pts: None,
                        detail: OverlayFindingKind::UnresolvedExpectation { reason },
                    },
                );
                report.overlays.push(OverlayElementSummary {
                    element: overlay.element,
                    kind: overlay.kind,
                    status: OverlayElementStatus::Unknown,
                    expected_frames: 0,
                    measurable_frames: 0,
                    unmeasurable_frames: 0,
                    checked_frames: 0,
                    animated_extreme_frames: 0,
                    measured_extreme_frames: 0,
                    boundary_checked_frames: 0,
                    failed_frames: 0,
                    worst_with_rms: 0.0,
                    min_without_rms: 0.0,
                });
            }
        }
    }

    if unresolved {
        report.row_result = ReviewRowResult::Unknown;
        return Ok(report);
    }

    let _no_locators = DenyLocatorOpens::enter();
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
            return Ok(report);
        }
    };

    if artifact_timing.video_frames < total_output_frames {
        report.push_finding(
            finding_cap,
            OverlayFinding {
                element: None,
                overlay_kind: None,
                frame_index: None,
                pts: None,
                detail: OverlayFindingKind::MissingFrame {
                    expected_frames: total_output_frames,
                    observed_frames: artifact_timing.video_frames,
                },
            },
        );
        report.row_result = ReviewRowResult::Failed;
        return Ok(report);
    }

    // Even when there are provably zero overlays on output, verify the first frame's raster and
    // full-stream frame count / PTS continuity so unreadable or dropped-frame encodes never pass.
    let mut active_by_frame: BTreeMap<u64, Vec<(usize, TitlePixelBounds)>> = BTreeMap::new();
    let mut boundary_by_frame: BTreeMap<u64, Vec<BoundaryProbe>> = BTreeMap::new();
    for (idx, schedule) in schedules.iter().enumerate() {
        for run in &schedule.runs {
            for frame in run.frames.clone() {
                active_by_frame
                    .entry(frame)
                    .or_default()
                    .push((idx, run.bounds));
            }
        }
        for probe in &schedule.boundary_probes {
            boundary_by_frame
                .entry(probe.outside_frame)
                .or_default()
                .push(BoundaryProbe {
                    schedule_index: idx,
                    ..*probe
                });
        }
    }

    let mut renderer = FrameRenderer::new(gpu);
    renderer.set_inputs(Some(Arc::clone(inputs)));
    renderer.set_lut_library(Arc::clone(&inputs.lut));
    renderer.set_export_stop(cancellation);
    // Keep cache strictly bounded to a handful of frames so residency never grows with caption count.
    let frame_working_bytes = usize::try_from(expectations.raster.0)
        .unwrap_or(0)
        .saturating_mul(usize::try_from(expectations.raster.1).unwrap_or(0))
        .saturating_mul(8);
    renderer.set_cache_budget(frame_working_bytes.saturating_mul(6).max(4 << 20));

    let mut states: Vec<OverlayRunState> = schedules.iter().map(OverlayRunState::new).collect();
    let mut observed_frames = 0_u64;
    let mut hard_failed = false;
    let mut stream_unknown = false;

    loop {
        if cancellation.is_cancelled() {
            return Err(MediaError::Cancelled);
        }
        let next_idx = decoder.next_frame_index();
        let needs_pixels =
            active_by_frame.contains_key(&next_idx) || boundary_by_frame.contains_key(&next_idx);
        let decoded = match decoder.next_frame_rgba64(needs_pixels, cancellation) {
            Ok(Some(frame)) => frame,
            Ok(None) => break,
            Err(MediaError::Cancelled) => return Err(MediaError::Cancelled),
            Err(error) => {
                stream_unknown = true;
                report.push_finding(
                    finding_cap,
                    OverlayFinding {
                        element: None,
                        overlay_kind: None,
                        frame_index: Some(next_idx),
                        pts: None,
                        detail: OverlayFindingKind::StreamUnreadable {
                            reason: error.to_string(),
                        },
                    },
                );
                break;
            }
        };
        observed_frames = observed_frames.saturating_add(1);
        let frame_idx = decoded.frame_index;
        let pts = decoded.pts;

        if decoded.dimensions != expectations.raster {
            hard_failed = true;
            report.push_finding(
                finding_cap,
                OverlayFinding {
                    element: None,
                    overlay_kind: None,
                    frame_index: Some(frame_idx),
                    pts: Some(pts),
                    detail: OverlayFindingKind::RasterMismatch {
                        expected: expectations.raster,
                        observed: decoded.dimensions,
                    },
                },
            );
            break;
        }

        match expected_frame_pts(artifact_timing, frame_idx) {
            Some(expected_pts) if expected_pts != pts => {
                hard_failed = true;
                report.push_finding(
                    finding_cap,
                    OverlayFinding {
                        element: None,
                        overlay_kind: None,
                        frame_index: Some(frame_idx),
                        pts: Some(pts),
                        detail: OverlayFindingKind::TimingMismatch {
                            expected_pts,
                            observed_pts: pts,
                        },
                    },
                );
                break;
            }
            None => {
                stream_unknown = true;
                report.push_finding(
                    finding_cap,
                    OverlayFinding {
                        element: None,
                        overlay_kind: None,
                        frame_index: Some(frame_idx),
                        pts: Some(pts),
                        detail: OverlayFindingKind::StreamUnreadable {
                            reason: "frame index overflowed recorded PTS grid".into(),
                        },
                    },
                );
                break;
            }
            Some(_) => {}
        }

        if !needs_pixels {
            continue;
        }

        let project_at = match output_frame_to_project_at(frame_idx, expectations, frozen_document)
        {
            Ok(at) => at,
            Err(reason) => {
                stream_unknown = true;
                report.push_finding(
                    finding_cap,
                    OverlayFinding {
                        element: None,
                        overlay_kind: None,
                        frame_index: Some(frame_idx),
                        pts: Some(pts),
                        detail: OverlayFindingKind::StreamUnreadable { reason },
                    },
                );
                break;
            }
        };

        let staged = match renderer.stage_delivery_layers(
            frozen_document,
            project_at,
            expectations.raster,
            RenderScale::FullResolution,
            DecodeStrategy::Sequential,
        ) {
            Ok(staged) => staged,
            Err(MediaError::Cancelled) => return Err(MediaError::Cancelled),
            Err(error) => {
                stream_unknown = true;
                report.push_finding(
                    finding_cap,
                    OverlayFinding {
                        element: None,
                        overlay_kind: None,
                        frame_index: Some(frame_idx),
                        pts: Some(pts),
                        detail: OverlayFindingKind::StreamUnreadable {
                            reason: format!("compositor layer staging failed: {error}"),
                        },
                    },
                );
                break;
            }
        };

        // Render the exact with-all-layers delivery frame ONCE per checked frame.
        let base_delivery = match renderer.render_staged_delivery(
            frozen_document,
            project_at,
            expectations.raster,
            &staged,
            None,
        ) {
            Ok(frame) => frame,
            Err(MediaError::Cancelled) => return Err(MediaError::Cancelled),
            Err(error) => {
                stream_unknown = true;
                report.push_finding(
                    finding_cap,
                    OverlayFinding {
                        element: None,
                        overlay_kind: None,
                        frame_index: Some(frame_idx),
                        pts: Some(pts),
                        detail: OverlayFindingKind::StreamUnreadable {
                            reason: format!("compositor reference render failed: {error}"),
                        },
                    },
                );
                break;
            }
        };

        let decoded_slice = DecodedRgba64 {
            bytes: decoded.rgba64le,
            full_u16_range: decoded.full_u16_range,
        };

        // 1. Check every overlay expected visible on this frame against with/without references.
        if let Some(entries) = active_by_frame.get(&frame_idx) {
            for &(sched_idx, bounds) in entries {
                if cancellation.is_cancelled() {
                    return Err(MediaError::Cancelled);
                }
                let schedule = &schedules[sched_idx];
                let without_delivery = match renderer.render_staged_delivery(
                    frozen_document,
                    project_at,
                    expectations.raster,
                    &staged,
                    Some(schedule.element),
                ) {
                    Ok(frame) => frame,
                    Err(MediaError::Cancelled) => return Err(MediaError::Cancelled),
                    Err(error) => {
                        stream_unknown = true;
                        report.push_finding(
                            finding_cap,
                            OverlayFinding {
                                element: Some(schedule.element),
                                overlay_kind: Some(schedule.kind),
                                frame_index: Some(frame_idx),
                                pts: Some(pts),
                                detail: OverlayFindingKind::StreamUnreadable {
                                    reason: format!(
                                        "without-overlay compositor render failed: {error}"
                                    ),
                                },
                            },
                        );
                        continue;
                    }
                };

                let outcome = compare_in_range_frame(
                    decoded_slice,
                    &base_delivery.rgba64le,
                    &without_delivery.rgba64le,
                    expectations.raster,
                    bounds,
                    config,
                );
                drop(without_delivery);

                let state = &mut states[sched_idx];
                match outcome {
                    InRangeOutcome::NotMeasurable => {
                        state.unmeasurable_frames = state.unmeasurable_frames.saturating_add(1);
                        report.unmeasurable_frames = report.unmeasurable_frames.saturating_add(1);
                    }
                    InRangeOutcome::Passed(metrics) => {
                        state.measurable_frames = state.measurable_frames.saturating_add(1);
                        state.record_metrics(metrics);
                        state.mark_extremes_measured(schedule, frame_idx);
                        report.checked_frames = report.checked_frames.saturating_add(1);
                    }
                    InRangeOutcome::Failed(metrics) => {
                        state.measurable_frames = state.measurable_frames.saturating_add(1);
                        state.failed_frames = state.failed_frames.saturating_add(1);
                        state.record_metrics(metrics);
                        report.checked_frames = report.checked_frames.saturating_add(1);
                        hard_failed = true;
                        report.push_finding(
                            finding_cap,
                            OverlayFinding {
                                element: Some(schedule.element),
                                overlay_kind: Some(schedule.kind),
                                frame_index: Some(frame_idx),
                                pts: Some(pts),
                                detail: OverlayFindingKind::CompositorReferenceMismatch {
                                    with_rms_tenths: to_tenths(metrics.with_rms),
                                    without_rms_tenths: to_tenths(metrics.without_rms),
                                    ref_rms_tenths: to_tenths(metrics.ref_rms),
                                    worst_tile_rms_tenths: to_tenths(metrics.worst_tile_rms),
                                },
                            },
                        );
                    }
                }
            }
        }

        // 2. Check outside-cue timing probes (`cue_start - 1` and `cue_end`) against `without` vs injected.
        if let Some(probes) = boundary_by_frame.get(&frame_idx) {
            for probe in probes {
                if cancellation.is_cancelled() {
                    return Err(MediaError::Cancelled);
                }
                // Exact upper-bound fast path: if even the top-`min_support_pixels` worst pixel
                // deviations between `decoded` and `base_delivery` inside `probe.bounds` have
                // RMS <= 0.65 * timing_max_without_rms, no support subset can exceed the leak
                // threshold under any injected overlay render.
                match boundary_probe_can_leak(
                    decoded_slice,
                    &base_delivery.rgba64le,
                    expectations.raster,
                    probe.bounds,
                    config,
                ) {
                    None => continue,
                    Some(false) => {
                        let state = &mut states[probe.schedule_index];
                        state.boundary_checked_frames =
                            state.boundary_checked_frames.saturating_add(1);
                        report.boundary_checked_frames =
                            report.boundary_checked_frames.saturating_add(1);
                        continue;
                    }
                    Some(true) => {}
                }
                let schedule = &schedules[probe.schedule_index];
                let rep_project_at = match output_frame_to_project_at(
                    probe.representative_in_cue_frame,
                    expectations,
                    frozen_document,
                ) {
                    Ok(at) => at,
                    Err(_) => continue,
                };
                let injected = match renderer.stage_single_clip_for_boundary(
                    frozen_document,
                    schedule.element,
                    rep_project_at,
                    expectations.raster,
                    RenderScale::FullResolution,
                    DecodeStrategy::Seek,
                ) {
                    Ok(Some(layer)) => layer,
                    Ok(None) => continue,
                    Err(MediaError::Cancelled) => return Err(MediaError::Cancelled),
                    Err(error) => {
                        stream_unknown = true;
                        report.push_finding(
                            finding_cap,
                            OverlayFinding {
                                element: Some(schedule.element),
                                overlay_kind: Some(schedule.kind),
                                frame_index: Some(frame_idx),
                                pts: Some(pts),
                                detail: OverlayFindingKind::StreamUnreadable {
                                    reason: format!(
                                        "outside-cue boundary layer staging failed: {error}"
                                    ),
                                },
                            },
                        );
                        continue;
                    }
                };
                let with_injected = match renderer.render_staged_delivery_with_injected(
                    frozen_document,
                    project_at,
                    expectations.raster,
                    &staged,
                    &injected,
                ) {
                    Ok(frame) => frame,
                    Err(MediaError::Cancelled) => return Err(MediaError::Cancelled),
                    Err(error) => {
                        stream_unknown = true;
                        report.push_finding(
                            finding_cap,
                            OverlayFinding {
                                element: Some(schedule.element),
                                overlay_kind: Some(schedule.kind),
                                frame_index: Some(frame_idx),
                                pts: Some(pts),
                                detail: OverlayFindingKind::StreamUnreadable {
                                    reason: format!("outside-cue boundary render failed: {error}"),
                                },
                            },
                        );
                        continue;
                    }
                };
                drop(injected);

                let outcome = compare_outside_cue_frame(
                    decoded_slice,
                    &base_delivery.rgba64le,
                    &with_injected.rgba64le,
                    expectations.raster,
                    probe.bounds,
                    config,
                );
                drop(with_injected);

                let state = &mut states[probe.schedule_index];
                match outcome {
                    OutsideCueOutcome::NotMeasurable => {}
                    OutsideCueOutcome::Absent => {
                        state.boundary_checked_frames =
                            state.boundary_checked_frames.saturating_add(1);
                        report.boundary_checked_frames =
                            report.boundary_checked_frames.saturating_add(1);
                    }
                    OutsideCueOutcome::UnexpectedOverlay {
                        without_rms,
                        with_injected_rms,
                        ref_rms,
                    } => {
                        state.boundary_checked_frames =
                            state.boundary_checked_frames.saturating_add(1);
                        state.failed_frames = state.failed_frames.saturating_add(1);
                        report.boundary_checked_frames =
                            report.boundary_checked_frames.saturating_add(1);
                        hard_failed = true;
                        report.push_finding(
                            finding_cap,
                            OverlayFinding {
                                element: Some(schedule.element),
                                overlay_kind: Some(schedule.kind),
                                frame_index: Some(frame_idx),
                                pts: Some(pts),
                                detail: OverlayFindingKind::UnexpectedOverlayOutsideCue {
                                    without_rms_tenths: to_tenths(without_rms),
                                    with_injected_rms_tenths: to_tenths(with_injected_rms),
                                    ref_rms_tenths: to_tenths(ref_rms),
                                },
                            },
                        );
                    }
                }
            }
        }
    }

    if !stream_unknown && observed_frames != total_output_frames {
        hard_failed = true;
        report.push_finding(
            finding_cap,
            OverlayFinding {
                element: None,
                overlay_kind: None,
                frame_index: Some(observed_frames),
                pts: None,
                detail: OverlayFindingKind::MissingFrame {
                    expected_frames: total_output_frames,
                    observed_frames,
                },
            },
        );
    }

    let mut any_unknown_element = false;
    for (schedule, state) in schedules.iter().zip(states.iter()) {
        let expected_extremes = schedule.extreme_buckets.len() as u64;
        let measured_extremes = state.measured_extreme_buckets.len() as u64;
        #[allow(clippy::cast_precision_loss)]
        let measurable_ratio = if schedule.expected_frames == 0 {
            0.0
        } else {
            state.measurable_frames as f32 / schedule.expected_frames as f32
        };
        let sufficient_measurability = state.measurable_frames >= 1
            && measurable_ratio >= config.min_measurable_frame_ratio
            && measured_extremes >= expected_extremes;

        let status = if state.failed_frames > 0 || hard_failed && state.measurable_frames == 0 {
            OverlayElementStatus::Failed
        } else if stream_unknown
            || state.measurable_frames + state.unmeasurable_frames < schedule.expected_frames
        {
            any_unknown_element = true;
            OverlayElementStatus::Unknown
        } else if !sufficient_measurability {
            any_unknown_element = true;
            report.push_finding(
                finding_cap,
                OverlayFinding {
                    element: Some(schedule.element),
                    overlay_kind: Some(schedule.kind),
                    frame_index: None,
                    pts: None,
                    detail: OverlayFindingKind::UnmeasurableOverlay {
                        expected_frames: schedule.expected_frames,
                        measurable_frames: state.measurable_frames,
                        expected_extremes,
                        measured_extremes,
                    },
                },
            );
            OverlayElementStatus::Unknown
        } else {
            OverlayElementStatus::Passed
        };

        report.animated_extreme_frames = report
            .animated_extreme_frames
            .saturating_add(measured_extremes);
        report.overlays.push(OverlayElementSummary {
            element: schedule.element,
            kind: schedule.kind,
            status,
            expected_frames: schedule.expected_frames,
            measurable_frames: state.measurable_frames,
            unmeasurable_frames: state.unmeasurable_frames,
            checked_frames: state.measurable_frames,
            animated_extreme_frames: expected_extremes,
            measured_extreme_frames: measured_extremes,
            boundary_checked_frames: state.boundary_checked_frames,
            failed_frames: state.failed_frames,
            worst_with_rms: state.worst_with_rms,
            min_without_rms: if state.seen_measurable {
                state.min_without_rms
            } else {
                0.0
            },
        });
    }

    report.checked_overlays = schedules.len();
    report.row_result = if hard_failed {
        ReviewRowResult::Failed
    } else if stream_unknown || any_unknown_element {
        ReviewRowResult::Unknown
    } else {
        ReviewRowResult::Pass
    };

    Ok(report)
}

pub(crate) fn expected_frame_pts(timing: &ReviewOutputTiming, frame_index: u64) -> Option<i64> {
    let step = timing.video_pts_step?;
    if step <= 0 {
        return None;
    }
    let offset = i64::try_from(frame_index).ok()?.checked_mul(step)?;
    timing.video_first_pts.checked_add(offset)
}

pub(crate) fn output_span_to_frames(
    span: &Range<ExactMicros>,
    output_fps: Rational,
    total_frames: u64,
) -> Option<Range<u64>> {
    let start = exact_micros_to_frame(span.start, output_fps, false)?;
    let end = exact_micros_to_frame(span.end, output_fps, true)?;
    let clamped_start = start.min(total_frames);
    let clamped_end = end.min(total_frames);
    if clamped_start >= clamped_end {
        return Some(clamped_start..clamped_start);
    }
    Some(clamped_start..clamped_end)
}

fn exact_micros_to_frame(time: ExactMicros, fps: Rational, ceil: bool) -> Option<u64> {
    let num = time.numerator().checked_mul(u128::from(fps.numerator()))?;
    let den = 1_000_000_u128
        .checked_mul(u128::from(fps.denominator()))?
        .checked_mul(u128::from(time.denominator()))?;
    if den == 0 {
        return None;
    }
    let frames = if ceil { num.div_ceil(den) } else { num / den };
    u64::try_from(frames).ok()
}

fn output_frame_to_project_at(
    frame_idx: u64,
    expectations: &ReviewExpectations,
    frozen_document: &Document,
) -> Result<TimeCode, String> {
    let output_tc = TimeCode(i64::try_from(frame_idx).unwrap_or(i64::MAX));
    let mapped = map_frames_with_rounding(
        output_tc,
        expectations.output_fps,
        expectations.project_fps,
        FrameRounding::Floor,
    )
    .map_err(|error| format!("output frame mapping failed: {error}"))?;
    let max_project = frozen_document.duration.0.saturating_sub(1).max(0);
    Ok(TimeCode(mapped.0.clamp(0, max_project)))
}

struct FrameBoundsRun {
    frames: Range<u64>,
    bounds: TitlePixelBounds,
}

#[derive(Clone, Copy)]
struct BoundaryProbe {
    schedule_index: usize,
    outside_frame: u64,
    representative_in_cue_frame: u64,
    bounds: TitlePixelBounds,
}

struct OverlaySchedule {
    element: ClipId,
    kind: OverlayKind,
    runs: Vec<FrameBoundsRun>,
    expected_frames: u64,
    /// Distinct animated extreme buckets (each bucket is the set of frames achieving that
    /// extreme; L14 requires at least one measurable frame in each bucket).
    extreme_buckets: Vec<BTreeSet<u64>>,
    boundary_probes: Vec<BoundaryProbe>,
}

struct OverlayRunState {
    measurable_frames: u64,
    unmeasurable_frames: u64,
    boundary_checked_frames: u64,
    failed_frames: u64,
    seen_measurable: bool,
    worst_with_rms: f32,
    min_without_rms: f32,
    measured_extreme_buckets: BTreeSet<usize>,
}

impl OverlayRunState {
    fn new(_schedule: &OverlaySchedule) -> Self {
        Self {
            measurable_frames: 0,
            unmeasurable_frames: 0,
            boundary_checked_frames: 0,
            failed_frames: 0,
            seen_measurable: false,
            worst_with_rms: 0.0,
            min_without_rms: f32::INFINITY,
            measured_extreme_buckets: BTreeSet::new(),
        }
    }

    fn record_metrics(&mut self, metrics: FrameMetrics) {
        self.seen_measurable = true;
        self.worst_with_rms = self.worst_with_rms.max(metrics.with_rms);
        self.min_without_rms = self.min_without_rms.min(metrics.without_rms);
    }

    fn mark_extremes_measured(&mut self, schedule: &OverlaySchedule, frame_idx: u64) {
        for (bucket_idx, frames) in schedule.extreme_buckets.iter().enumerate() {
            if frames.contains(&frame_idx) {
                self.measured_extreme_buckets.insert(bucket_idx);
            }
        }
    }
}

fn find_clip(document: &Document, id: ClipId) -> Option<&Clip> {
    document
        .tracks
        .iter()
        .filter(|track| track.kind == TrackKind::Video)
        .flat_map(|track| track.clips.iter())
        .find(|clip| clip.id == id)
}

fn build_overlay_schedule(
    overlay: &ReviewOverlay,
    expectations: &ReviewExpectations,
    frozen_document: &Document,
    total_output_frames: u64,
) -> Result<OverlaySchedule, String> {
    if overlay.visible.is_empty() {
        return Err("overlay has no visible runs".into());
    }

    let Some(clip) = find_clip(frozen_document, overlay.element) else {
        return Err(format!(
            "overlay clip {} is missing from frozen document",
            overlay.element
        ));
    };

    match &overlay.pixels {
        OverlayPixels::RenderedText => {
            let OverlayText::Expected(expected_text) = &overlay.text else {
                return Err("text overlay lacks resolved expected text".into());
            };
            if expected_text.trim().is_empty() {
                return Err("text overlay expected text is empty".into());
            }
            let ClipContent::Title(title) = &clip.content else {
                return Err("text overlay does not map to a Title clip in frozen document".into());
            };
            if &title.text != expected_text {
                return Err(format!(
                    "frozen document title text disagrees with expectation record on clip {}",
                    overlay.element
                ));
            }
        }
        OverlayPixels::PinnedImage { .. } => {
            if !matches!(clip.content, ClipContent::Media | ClipContent::Freeze(_)) {
                return Err(
                    "pinned card overlay does not map to a Media/Freeze clip in frozen document"
                        .into(),
                );
            }
        }
        OverlayPixels::Unknown(reason) => {
            return Err(format!("overlay pixels unresolved: {reason:?}"));
        }
    }

    let mut runs = Vec::with_capacity(overlay.visible.len());
    let mut visible_frame_set = BTreeSet::new();
    for run in &overlay.visible {
        let bounds = match run.bounds {
            Ok(bounds) => bounds,
            Err(reason) => return Err(format!("overlay bounds unresolved: {reason:?}")),
        };
        let frames =
            output_span_to_frames(&run.output, expectations.output_fps, total_output_frames)
                .ok_or_else(|| "overlay output span overflowed frame grid".to_owned())?;
        if frames.is_empty() {
            continue;
        }
        if bounds.right <= bounds.left || bounds.bottom <= bounds.top {
            return Err("overlay bounds are empty".into());
        }
        if bounds.left < 0
            || bounds.top < 0
            || bounds.right > i32::try_from(expectations.raster.0).unwrap_or(i32::MAX)
            || bounds.bottom > i32::try_from(expectations.raster.1).unwrap_or(i32::MAX)
        {
            return Err("overlay bounds extend outside output raster".into());
        }
        for frame in frames.clone() {
            visible_frame_set.insert(frame);
        }
        runs.push(FrameBoundsRun { frames, bounds });
    }
    if runs.is_empty() {
        return Err("overlay has zero in-range output frames".into());
    }

    let expected_frames = visible_frame_set.len() as u64;
    let extreme_buckets = compute_extreme_buckets(&runs);
    let boundary_probes = compute_boundary_probes(&runs, &visible_frame_set, total_output_frames);

    Ok(OverlaySchedule {
        element: overlay.element,
        kind: overlay.kind,
        runs,
        expected_frames,
        extreme_buckets,
        boundary_probes,
    })
}

fn compute_extreme_buckets(runs: &[FrameBoundsRun]) -> Vec<BTreeSet<u64>> {
    if runs.len() <= 1 {
        return Vec::new();
    }
    let first = runs[0].bounds;
    if runs.iter().all(|run| run.bounds == first) {
        return Vec::new();
    }

    let extractors: [fn(TitlePixelBounds) -> i32; 4] =
        [|b| b.left, |b| b.right, |b| b.top, |b| b.bottom];
    let size_extractors: [fn(TitlePixelBounds) -> i32; 2] =
        [|b| b.right - b.left, |b| b.bottom - b.top];

    let mut buckets: Vec<BTreeSet<u64>> = Vec::new();
    let mut push_bucket = |set: BTreeSet<u64>| {
        if !set.is_empty() && !buckets.contains(&set) {
            buckets.push(set);
        }
    };

    for f in extractors.into_iter().chain(size_extractors) {
        let min_v = runs.iter().map(|r| f(r.bounds)).min().unwrap();
        let max_v = runs.iter().map(|r| f(r.bounds)).max().unwrap();
        if min_v == max_v {
            continue;
        }
        let mut min_frames = BTreeSet::new();
        let mut max_frames = BTreeSet::new();
        for run in runs {
            let v = f(run.bounds);
            if v == min_v {
                min_frames.extend(run.frames.clone());
            }
            if v == max_v {
                max_frames.extend(run.frames.clone());
            }
        }
        push_bucket(min_frames);
        push_bucket(max_frames);
    }
    buckets
}

fn compute_boundary_probes(
    runs: &[FrameBoundsRun],
    visible_frames: &BTreeSet<u64>,
    total_output_frames: u64,
) -> Vec<BoundaryProbe> {
    // Merge contiguous visible runs into cue spans so we probe only just before each cue's start
    // and just after each cue's end.
    let mut sorted_indices: Vec<usize> = (0..runs.len()).collect();
    sorted_indices.sort_unstable_by_key(|&i| runs[i].frames.start);

    let mut spans: Vec<(u64, u64, TitlePixelBounds, TitlePixelBounds)> = Vec::new();
    for idx in sorted_indices {
        let run = &runs[idx];
        if let Some(last) = spans.last_mut()
            && run.frames.start <= last.1
        {
            if run.frames.end > last.1 {
                last.1 = run.frames.end;
                last.3 = run.bounds;
            }
        } else {
            spans.push((run.frames.start, run.frames.end, run.bounds, run.bounds));
        }
    }

    let mut probes = Vec::new();
    for (start, end, start_bounds, end_bounds) in spans {
        let mid_frame = start + (end - start) / 2;
        let mid_bounds = runs
            .iter()
            .find(|r| r.frames.contains(&mid_frame))
            .map(|r| r.bounds);
        if let Some(before) = start.checked_sub(1)
            && !visible_frames.contains(&before)
        {
            // When bounds are stationary across the cue, use the midpoint so fade-in cues still
            // produce a full-opacity injected probe; for moving overlays use the exact edge frame.
            let rep = if end - start >= 3 && mid_bounds == Some(start_bounds) {
                mid_frame
            } else {
                start
            };
            probes.push(BoundaryProbe {
                schedule_index: 0,
                outside_frame: before,
                representative_in_cue_frame: rep,
                bounds: start_bounds,
            });
        }
        if end < total_output_frames && !visible_frames.contains(&end) {
            let rep = if end - start >= 3 && mid_bounds == Some(end_bounds) {
                mid_frame
            } else {
                end - 1
            };
            probes.push(BoundaryProbe {
                schedule_index: 0,
                outside_frame: end,
                representative_in_cue_frame: rep,
                bounds: end_bounds,
            });
        }
    }
    probes
}

#[derive(Clone, Copy)]
struct DecodedRgba64<'a> {
    bytes: &'a [u8],
    full_u16_range: bool,
}

#[derive(Clone, Copy)]
struct FrameMetrics {
    with_rms: f32,
    without_rms: f32,
    ref_rms: f32,
    worst_tile_rms: f32,
}

enum InRangeOutcome {
    NotMeasurable,
    Passed(FrameMetrics),
    Failed(FrameMetrics),
}

enum OutsideCueOutcome {
    NotMeasurable,
    Absent,
    UnexpectedOverlay {
        without_rms: f32,
        with_injected_rms: f32,
        ref_rms: f32,
    },
}

#[inline]
fn rgba64_pixel_to_ycbcr(bytes: &[u8], pixel_index: usize, full_u16_range: bool) -> [f32; 3] {
    let off = pixel_index * 8;
    let scale = if full_u16_range {
        255.0 / 65_535.0
    } else {
        1.0 / 256.0
    };
    let r = f32::from(u16::from_le_bytes([bytes[off], bytes[off + 1]])) * scale;
    let g = f32::from(u16::from_le_bytes([bytes[off + 2], bytes[off + 3]])) * scale;
    let b = f32::from(u16::from_le_bytes([bytes[off + 4], bytes[off + 5]])) * scale;
    let y = 0.2126 * r + 0.7152 * g + 0.0722 * b;
    let cb = -0.114_572 * r - 0.385_428 * g + 0.5 * b;
    let cr = 0.5 * r - 0.454_153 * g - 0.045_847 * b;
    [y, cb, cr]
}

#[inline]
fn ycbcr_dist_sq(a: [f32; 3], b: [f32; 3]) -> f32 {
    let dy = a[0] - b[0];
    let dcb = a[1] - b[1];
    let dcr = a[2] - b[2];
    dy * dy + 0.25 * (dcb * dcb + dcr * dcr)
}

fn padded_box(bounds: TitlePixelBounds, raster: (u32, u32)) -> (usize, usize, usize, usize) {
    let rw = i32::try_from(raster.0).unwrap_or(i32::MAX);
    let rh = i32::try_from(raster.1).unwrap_or(i32::MAX);
    let x0 = (bounds.left - BOX_PAD_PX).clamp(0, rw) as usize;
    let y0 = (bounds.top - BOX_PAD_PX).clamp(0, rh) as usize;
    let x1 = (bounds.right + BOX_PAD_PX).clamp(0, rw) as usize;
    let y1 = (bounds.bottom + BOX_PAD_PX).clamp(0, rh) as usize;
    (x0, y0, x1, y1)
}

#[allow(clippy::cast_precision_loss)]
fn compare_in_range_frame(
    decoded: DecodedRgba64<'_>,
    with_ref: &[u8],
    without_ref: &[u8],
    raster: (u32, u32),
    bounds: TitlePixelBounds,
    config: &OverlayVerificationConfig,
) -> InRangeOutcome {
    let (x0, y0, x1, y1) = padded_box(bounds, raster);
    if x1 <= x0 || y1 <= y0 {
        return InRangeOutcome::NotMeasurable;
    }
    let width = raster.0 as usize;
    let box_w = x1 - x0;
    let box_h = y1 - y0;
    let min_sig_sq = config.min_pixel_signal * config.min_pixel_signal;

    // Store compact per-pixel squared distances inside the bounded overlay box only (no full-frame
    // buffers retained across elements or frames).
    let pixel_count = box_w.saturating_mul(box_h);
    let mut ref_sq = Vec::with_capacity(pixel_count);
    let mut with_sq = Vec::with_capacity(pixel_count);
    let mut without_sq = Vec::with_capacity(pixel_count);

    let mut support = 0_usize;
    let mut sum_ref_sq = 0.0_f64;
    let mut sum_with_sq = 0.0_f64;
    let mut sum_without_sq = 0.0_f64;

    for y in y0..y1 {
        let row_base = y * width;
        for x in x0..x1 {
            let px = row_base + x;
            let d = rgba64_pixel_to_ycbcr(decoded.bytes, px, decoded.full_u16_range);
            let w = rgba64_pixel_to_ycbcr(with_ref, px, false);
            let b = rgba64_pixel_to_ycbcr(without_ref, px, false);

            let r2 = ycbcr_dist_sq(w, b);
            let w2 = ycbcr_dist_sq(d, w);
            let b2 = ycbcr_dist_sq(d, b);

            ref_sq.push(r2);
            with_sq.push(w2);
            without_sq.push(b2);

            if r2 >= min_sig_sq {
                support += 1;
                sum_ref_sq += f64::from(r2);
                sum_with_sq += f64::from(w2);
                sum_without_sq += f64::from(b2);
            }
        }
    }

    if support < config.min_support_pixels {
        return InRangeOutcome::NotMeasurable;
    }
    let ref_rms = (sum_ref_sq / support as f64).sqrt() as f32;
    if ref_rms < config.min_reference_separation {
        return InRangeOutcome::NotMeasurable;
    }

    let with_rms = (sum_with_sq / support as f64).sqrt() as f32;
    let without_rms = (sum_without_sq / support as f64).sqrt() as f32;

    // Localized overlapping tile scan inside the overlay box to catch single-digit/word
    // substitutions ("1,250" -> "1,850") and localized extra ink.
    let tile_w = TILE_SIZE.min(box_w);
    let tile_h = TILE_SIZE.min(box_h);
    let step_x = TILE_STEP.min(tile_w).max(1);
    let step_y = TILE_STEP.min(tile_h).max(1);
    let mut worst_tile_rms = 0.0_f32;
    let mut tile_failed = false;

    let mut ty = 0;
    loop {
        let y_end = (ty + tile_h).min(box_h);
        let y_start = y_end.saturating_sub(tile_h);
        let mut tx = 0;
        loop {
            let x_end = (tx + tile_w).min(box_w);
            let x_start = x_end.saturating_sub(tile_w);

            let mut tile_n = 0_usize;
            let mut tile_all_with_sq = 0.0_f64;
            let mut tile_sup = 0_usize;
            let mut tile_sup_ref_sq = 0.0_f64;
            let mut tile_sup_with_sq = 0.0_f64;
            let mut tile_sup_without_sq = 0.0_f64;

            for ly in y_start..y_end {
                let row = ly * box_w;
                for lx in x_start..x_end {
                    let i = row + lx;
                    let r2 = ref_sq[i];
                    let w2 = with_sq[i];
                    tile_n += 1;
                    tile_all_with_sq += f64::from(w2);
                    if r2 >= min_sig_sq {
                        tile_sup += 1;
                        tile_sup_ref_sq += f64::from(r2);
                        tile_sup_with_sq += f64::from(w2);
                        tile_sup_without_sq += f64::from(without_sq[i]);
                    }
                }
            }

            if tile_n > 0 {
                let tile_rms = (tile_all_with_sq / tile_n as f64).sqrt() as f32;
                worst_tile_rms = worst_tile_rms.max(tile_rms);
                if tile_rms > config.max_tile_error_rms {
                    tile_failed = true;
                }
            }
            if tile_sup >= TILE_MIN_SUPPORT_PIXELS {
                let t_ref = (tile_sup_ref_sq / tile_sup as f64).sqrt() as f32;
                if t_ref >= config.min_reference_separation {
                    let t_with = (tile_sup_with_sq / tile_sup as f64).sqrt() as f32;
                    let t_without = (tile_sup_without_sq / tile_sup as f64).sqrt() as f32;
                    let tile_ceiling = config
                        .max_tile_error_rms
                        .max(t_ref * (config.max_relative_error * 0.75));
                    if t_with > tile_ceiling || t_with > t_without * 0.72 {
                        tile_failed = true;
                    }
                }
            }

            if x_end >= box_w {
                break;
            }
            tx += step_x;
        }
        if y_end >= box_h {
            break;
        }
        ty += step_y;
    }

    let metrics = FrameMetrics {
        with_rms,
        without_rms,
        ref_rms,
        worst_tile_rms,
    };

    let global_ceiling = config
        .max_codec_error_rms
        .max(ref_rms * config.max_relative_error);
    let matches_with = with_rms <= global_ceiling
        && with_rms <= without_rms * config.max_with_to_without_ratio
        && (without_rms - with_rms) >= config.min_advantage_margin
        && !tile_failed;

    if matches_with {
        InRangeOutcome::Passed(metrics)
    } else {
        InRangeOutcome::Failed(metrics)
    }
}

#[allow(clippy::cast_precision_loss)]
fn boundary_probe_can_leak(
    decoded: DecodedRgba64<'_>,
    without_ref: &[u8],
    raster: (u32, u32),
    bounds: TitlePixelBounds,
    config: &OverlayVerificationConfig,
) -> Option<bool> {
    let (x0, y0, x1, y1) = padded_box(bounds, raster);
    if x1 <= x0 || y1 <= y0 {
        return None;
    }
    let m = config.min_support_pixels.max(1);
    let box_pixels = (x1 - x0).saturating_mul(y1 - y0);
    if box_pixels < m {
        return None;
    }
    let width = raster.0 as usize;
    let mut top = BinaryHeap::<Reverse<u32>>::with_capacity(m + 1);
    for y in y0..y1 {
        let row_base = y * width;
        for x in x0..x1 {
            let px = row_base + x;
            let d = rgba64_pixel_to_ycbcr(decoded.bytes, px, decoded.full_u16_range);
            let b = rgba64_pixel_to_ycbcr(without_ref, px, false);
            let d2 = ycbcr_dist_sq(d, b);
            let bits = d2.to_bits();
            if top.len() < m {
                top.push(Reverse(bits));
            } else if let Some(&Reverse(min_bits)) = top.peek()
                && bits > min_bits
            {
                top.pop();
                top.push(Reverse(bits));
            }
        }
    }
    let sum_top: f64 = top
        .into_iter()
        .map(|Reverse(bits)| f64::from(f32::from_bits(bits)))
        .sum();
    let max_possible_without_rms = (sum_top / m as f64).sqrt() as f32;
    Some(max_possible_without_rms > config.timing_max_without_rms * 0.65)
}

#[allow(clippy::cast_precision_loss)]
fn compare_outside_cue_frame(
    decoded: DecodedRgba64<'_>,
    without_ref: &[u8],
    with_injected_ref: &[u8],
    raster: (u32, u32),
    bounds: TitlePixelBounds,
    config: &OverlayVerificationConfig,
) -> OutsideCueOutcome {
    let (x0, y0, x1, y1) = padded_box(bounds, raster);
    if x1 <= x0 || y1 <= y0 {
        return OutsideCueOutcome::NotMeasurable;
    }
    let width = raster.0 as usize;
    let min_sig_sq = config.min_pixel_signal * config.min_pixel_signal;

    let mut support = 0_usize;
    let mut sum_ref_sq = 0.0_f64;
    let mut sum_without_sq = 0.0_f64;
    let mut sum_with_sq = 0.0_f64;
    let mut dot_proj = 0.0_f64;

    for y in y0..y1 {
        let row_base = y * width;
        for x in x0..x1 {
            let px = row_base + x;
            let d = rgba64_pixel_to_ycbcr(decoded.bytes, px, decoded.full_u16_range);
            let b = rgba64_pixel_to_ycbcr(without_ref, px, false);
            let w = rgba64_pixel_to_ycbcr(with_injected_ref, px, false);

            let r2 = ycbcr_dist_sq(w, b);
            if r2 < min_sig_sq {
                continue;
            }
            support += 1;
            sum_ref_sq += f64::from(r2);
            sum_without_sq += f64::from(ycbcr_dist_sq(d, b));
            sum_with_sq += f64::from(ycbcr_dist_sq(d, w));

            // Weighted inner product <(D - B), (W - B)> detects even partial-opacity early/lingering cues.
            let p_y = f64::from(d[0] - b[0]) * f64::from(w[0] - b[0]);
            let p_cb = 0.25 * f64::from(d[1] - b[1]) * f64::from(w[1] - b[1]);
            let p_cr = 0.25 * f64::from(d[2] - b[2]) * f64::from(w[2] - b[2]);
            dot_proj += p_y + p_cb + p_cr;
        }
    }

    if support < config.min_support_pixels {
        return OutsideCueOutcome::NotMeasurable;
    }
    let ref_rms = (sum_ref_sq / support as f64).sqrt() as f32;
    if ref_rms < config.min_reference_separation {
        return OutsideCueOutcome::NotMeasurable;
    }
    let without_rms = (sum_without_sq / support as f64).sqrt() as f32;
    let with_injected_rms = (sum_with_sq / support as f64).sqrt() as f32;
    let projection_ratio = if sum_ref_sq > 0.0 {
        (dot_proj / sum_ref_sq) as f32
    } else {
        0.0
    };

    let leaked = (without_rms > config.timing_max_without_rms && with_injected_rms <= without_rms)
        || (without_rms > config.timing_max_without_rms * 0.65 && projection_ratio >= 0.35);

    if leaked {
        OutsideCueOutcome::UnexpectedOverlay {
            without_rms,
            with_injected_rms,
            ref_rms,
        }
    } else {
        OutsideCueOutcome::Absent
    }
}

#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn to_tenths(v: f32) -> u16 {
    (v.max(0.0) * 10.0).round().clamp(0.0, f32::from(u16::MAX)) as u16
}

// ---------------------------------------------------------------------------
// Shared geometry helpers used by review_stills reference rendering
// ---------------------------------------------------------------------------

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

#[allow(clippy::cast_precision_loss)]
pub(crate) fn quad_covers_pixel(p: &LayerParams, size: [f32; 2], pixel: [usize; 2]) -> bool {
    let ndc = [
        (pixel[0] as f32 + 0.5) / size[0] * 2.0 - 1.0,
        1.0 - (pixel[1] as f32 + 0.5) / size[1] * 2.0,
    ];
    let uv = quad_uv(p, ndc);
    uv[0] >= 0.0 && uv[0] <= 1.0 && uv[1] >= 0.0 && uv[1] <= 1.0
}

#[allow(clippy::cast_precision_loss)]
pub(crate) fn transition_covers_pixel(p: &LayerParams, size: [f32; 2], pixel: [usize; 2]) -> bool {
    if p.coverage_on <= 0.5 {
        return true;
    }
    let axis = usize::from(p.coverage_axis > 0.5);
    let centre = pixel[axis] as f64 + 0.5;
    let below = centre < f64::from(p.coverage_edge) * f64::from(size[axis]);
    below == (p.coverage_on < 1.5)
}

#[allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss
)]
pub(crate) fn sample_rgba_bilinear(src: &[u8], w: u32, h: u32, uv: [f32; 2]) -> [u8; 4] {
    if w == 0 || h == 0 || src.len() < (w as usize * h as usize * 4) {
        return [0; 4];
    }
    let fx = (uv[0] * w as f32 - 0.5).clamp(0.0, (w - 1) as f32);
    let fy = (uv[1] * h as f32 - 0.5).clamp(0.0, (h - 1) as f32);
    let x0 = fx.floor() as usize;
    let y0 = fy.floor() as usize;
    let x1 = (x0 + 1).min(w as usize - 1);
    let y1 = (y0 + 1).min(h as usize - 1);
    let tx = fx - x0 as f32;
    let ty = fy - y0 as f32;
    let stride = w as usize;
    let mut out = [0_u8; 4];
    for c in 0..4 {
        let p00 = f32::from(src[(y0 * stride + x0) * 4 + c]);
        let p10 = f32::from(src[(y0 * stride + x1) * 4 + c]);
        let p01 = f32::from(src[(y1 * stride + x0) * 4 + c]);
        let p11 = f32::from(src[(y1 * stride + x1) * 4 + c]);
        let top = p00 + (p10 - p00) * tx;
        let bot = p01 + (p11 - p01) * tx;
        out[c] = (top + (bot - top) * ty).round().clamp(0.0, 255.0) as u8;
    }
    out
}

/// Decode a single verified still-image stream (PNG/JPEG/etc.) into RGBA8.
pub(crate) fn decode_still_image_rgba8(
    reader: Box<dyn ReviewReadSeek>,
    cancellation: &ExportCancellation,
) -> Result<(u32, u32, Arc<Vec<u8>>), MediaError> {
    if cancellation.is_cancelled() {
        return Err(MediaError::Cancelled);
    }
    crate::quiet_ffmpeg_log();
    let io = ffmpeg::format::context::StreamIo::from_read_seek(reader).map_err(backend)?;
    let cancel = cancellation.clone();
    let mut input = ffmpeg::format::input_from_stream_with_interrupt(io, None, None, move || {
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
    let mut eof_sent = false;
    loop {
        if cancellation.is_cancelled() {
            return Err(MediaError::Cancelled);
        }
        match decoder.receive_frame(&mut frame) {
            Ok(()) => {
                got_frame = true;
                break;
            }
            Err(ffmpeg::Error::Eof) => break,
            Err(ffmpeg::Error::Other { errno }) if errno == ffmpeg::error::EAGAIN => {
                if eof_sent {
                    break;
                }
                let mut packet = ffmpeg::Packet::empty();
                match packet.read(&mut input) {
                    Ok(()) if packet.stream() == stream_index => {
                        decoder.send_packet(&packet).map_err(backend)?;
                    }
                    Ok(()) => {}
                    Err(ffmpeg::Error::Eof) => {
                        decoder.send_eof().map_err(backend)?;
                        eof_sent = true;
                    }
                    Err(error) => return Err(backend(error)),
                }
            }
            Err(error) => return Err(backend(error)),
        }
    }
    if !got_frame {
        return Err(MediaError::Backend(
            "pinned card image decoded zero frames".into(),
        ));
    }
    let width = frame.width();
    let height = frame.height();
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
    Ok((width, height, Arc::new(rgba8)))
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

pub(crate) struct DecodedReviewVideoFrame64<'a> {
    pub(crate) frame_index: u64,
    pub(crate) pts: i64,
    pub(crate) dimensions: (u32, u32),
    pub(crate) rgba64le: &'a [u8],
    pub(crate) full_u16_range: bool,
}

/// Bounded streaming/seekable decoder over a verified review artifact [`File`].
/// Keeps at most one converted frame resident in memory.
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
    rgba64le: Vec<u8>,
    frames_decoded: u64,
    seeks_performed: u64,
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
            rgba64le: Vec::new(),
            frames_decoded: 0,
            seeks_performed: 0,
        })
    }

    pub(crate) const fn next_frame_index(&self) -> u64 {
        self.next_index
    }

    pub(crate) const fn frames_decoded(&self) -> u64 {
        self.frames_decoded
    }

    pub(crate) const fn seeks_performed(&self) -> u64 {
        self.seeks_performed
    }

    /// Seek to the keyframe at or before `target_frame` using the recorded PTS grid.
    pub(crate) fn seek_to_frame(
        &mut self,
        target_frame: u64,
        timing: &ReviewOutputTiming,
        cancellation: &ExportCancellation,
    ) -> Result<(), MediaError> {
        if cancellation.is_cancelled() {
            return Err(MediaError::Cancelled);
        }
        let target_pts = expected_frame_pts(timing, target_frame).ok_or_else(|| {
            MediaError::Backend(
                "cannot seek review artifact without valid recorded PTS step".into(),
            )
        })?;
        let tb_num = i128::from(self.stream_time_base.numerator());
        let tb_den = i128::from(self.stream_time_base.denominator());
        if tb_num <= 0 || tb_den <= 0 {
            return Err(MediaError::Backend(
                "review video stream time base is invalid".into(),
            ));
        }
        let global_us = i64::try_from(
            (i128::from(target_pts)
                .saturating_mul(1_000_000)
                .saturating_mul(tb_num))
                / tb_den,
        )
        .unwrap_or(i64::MAX);
        self.input.seek(global_us, ..global_us).map_err(|error| {
            if cancellation.is_cancelled() {
                MediaError::Cancelled
            } else {
                backend(error)
            }
        })?;
        self.decoder.flush();
        self.eof_sent = false;
        self.eof_reached = false;
        self.seeks_performed = self.seeks_performed.saturating_add(1);
        Ok(())
    }

    /// Seek back to stream start (fallback if an open-GOP keyframe seek landed past target).
    pub(crate) fn seek_to_start(
        &mut self,
        cancellation: &ExportCancellation,
    ) -> Result<(), MediaError> {
        if cancellation.is_cancelled() {
            return Err(MediaError::Cancelled);
        }
        self.input.seek(0, ..0).map_err(|error| {
            if cancellation.is_cancelled() {
                MediaError::Cancelled
            } else {
                backend(error)
            }
        })?;
        self.decoder.flush();
        self.eof_sent = false;
        self.eof_reached = false;
        self.next_index = 0;
        self.seeks_performed = self.seeks_performed.saturating_add(1);
        Ok(())
    }

    /// Decode the next raw video frame, returning `(pts, dimensions, frame)` without running swscale.
    fn receive_next_raw(
        &mut self,
        cancellation: &ExportCancellation,
    ) -> Result<Option<(i64, (u32, u32), ffmpeg::frame::Video)>, MediaError> {
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
                    self.frames_decoded = self.frames_decoded.saturating_add(1);
                    return Ok(Some((pts, dimensions, decoded)));
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
                        Err(error) => {
                            if cancellation.is_cancelled() {
                                return Err(MediaError::Cancelled);
                            }
                            return Err(backend(error));
                        }
                    }
                }
                Err(error) => {
                    if cancellation.is_cancelled() {
                        return Err(MediaError::Cancelled);
                    }
                    return Err(backend(error));
                }
            }
        }
    }

    fn ensure_graph(&mut self, pixel: ffmpeg::format::Pixel) -> Result<(), MediaError> {
        if self.graph.is_none() {
            let mut description = color_description_from_decoder(&self.decoder, Some(pixel));
            let is_rgb = is_rgb_pixel(pixel);
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
        Ok(())
    }

    pub(crate) fn next_frame_rgba64(
        &mut self,
        convert_rgba64: bool,
        cancellation: &ExportCancellation,
    ) -> Result<Option<DecodedReviewVideoFrame64<'_>>, MediaError> {
        let Some((pts, dimensions, decoded)) = self.receive_next_raw(cancellation)? else {
            return Ok(None);
        };
        let frame_index = self.next_index;
        self.next_index = self.next_index.saturating_add(1);
        let mut full_u16_range = false;
        if convert_rgba64 {
            self.ensure_graph(decoded.format())?;
            let (graph, is_rgb) = self.graph.as_mut().expect("initialized graph");
            full_u16_range = *is_rgb;
            scale_frame_to_rgba64le(graph, decoded, &mut self.filter_pts, &mut self.rgba64le)?;
        }
        Ok(Some(DecodedReviewVideoFrame64 {
            frame_index,
            pts,
            dimensions,
            rgba64le: &self.rgba64le,
            full_u16_range,
        }))
    }

    /// Decode the next frame after a seek, deriving its exact `frame_index` from `timing`'s PTS
    /// grid and converting to RGBA8 only when `should_convert(frame_index)` is true.
    pub(crate) fn next_frame_by_pts(
        &mut self,
        timing: &ReviewOutputTiming,
        should_convert: impl FnOnce(u64) -> bool,
        cancellation: &ExportCancellation,
    ) -> Result<Option<DecodedReviewVideoFrame<'_>>, MediaError> {
        let step = timing.video_pts_step.filter(|&s| s > 0).ok_or_else(|| {
            MediaError::Backend("recorded video frame PTS step is unknown or non-positive".into())
        })?;
        let Some((pts, dimensions, decoded)) = self.receive_next_raw(cancellation)? else {
            return Ok(None);
        };
        let delta = pts
            .checked_sub(timing.video_first_pts)
            .ok_or_else(|| MediaError::Backend("decoded review frame PTS underflow".into()))?;
        if delta < 0 || delta % step != 0 {
            return Err(MediaError::Backend(format!(
                "decoded review frame PTS {pts} is off recorded grid (first={}, step={step})",
                timing.video_first_pts
            )));
        }
        let frame_index = u64::try_from(delta / step)
            .map_err(|_| MediaError::Backend("decoded review frame index overflow".into()))?;
        self.next_index = frame_index.saturating_add(1);
        if should_convert(frame_index) {
            self.ensure_graph(decoded.format())?;
            let (graph, is_rgb) = self.graph.as_mut().expect("initialized graph");
            scale_frame_to_rgba8(
                graph,
                decoded,
                &mut self.filter_pts,
                *is_rgb,
                &mut self.rgba8,
            )?;
        }
        Ok(Some(DecodedReviewVideoFrame {
            frame_index,
            pts,
            dimensions,
            rgba8: &self.rgba8,
        }))
    }
}

fn scale_frame_to_rgba64le(
    graph: &mut ffmpeg::filter::Graph,
    mut frame: ffmpeg::frame::Video,
    filter_pts: &mut i64,
    out_rgba64le: &mut Vec<u8>,
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
    let row_bytes = width.saturating_mul(8);
    let stride = converted.stride(0);
    let data = converted.data(0);
    out_rgba64le.clear();
    out_rgba64le.reserve(row_bytes.saturating_mul(height));
    for row in 0..height {
        let start = row.saturating_mul(stride);
        let end = start.saturating_add(row_bytes);
        out_rgba64le.extend_from_slice(&data[start..end]);
    }
    Ok(())
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
