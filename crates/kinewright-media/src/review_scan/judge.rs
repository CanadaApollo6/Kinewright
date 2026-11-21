//! Pure judgement of a decoded summary against the frozen expectations.
use std::ops::Range;

use kinewright_core::output_review::{
    RequiredReviewRow, ReviewExpectations, ReviewLane, ReviewRowResult, ReviewSegment, ReviewSpan,
    SegmentSampling,
};
use kinewright_core::{DeliveryTagSource, QaSeverity, Rational};

use super::decode::{AudioSummary, ScanSummary, TimingEvent, VideoSummary};
use super::{
    FindingKind, FindingVerdict, ReviewRowReport, ReviewScanInputs, SCAN_HOP_MICROS,
    TechnicalReviewReport, TechnicalScanConfig,
};

use FindingVerdict::{Failed, Undecided};

/// Only the structure row ran; the other two are unrun.
pub(crate) fn unrun(config: &TechnicalScanConfig) -> TechnicalReviewReport {
    let mut loudness = ReviewRowReport::new(RequiredReviewRow::LoudnessAndPeak);
    loudness.result = ReviewRowResult::Unrun;
    let mut defects = ReviewRowReport::new(RequiredReviewRow::TechnicalDefectScan);
    defects.result = ReviewRowResult::Unrun;
    TechnicalReviewReport {
        configuration_sha256: config.sha256(),
        structure: ReviewRowReport::new(RequiredReviewRow::StructureAndAvIntegrity),
        loudness,
        defects,
        decoded_frames: 0,
        decoded_audio_samples: 0,
        measured: None,
    }
}

/// Output clock helpers on the expected frame grid.
pub(crate) struct Clock {
    fps: Rational,
}

impl Clock {
    pub(crate) fn new(fps: Rational) -> Self {
        Self { fps }
    }

    /// Start of output frame `index`, in microseconds.
    pub(crate) fn frame_us(&self, index: u64) -> u64 {
        let value = u128::from(index) * 1_000_000 * u128::from(self.fps.denominator())
            / u128::from(self.fps.numerator().max(1));
        u64::try_from(value).unwrap_or(u64::MAX)
    }

    /// The frame presented at `us`.
    pub(crate) fn frame_at(&self, us: u64) -> u64 {
        let value = u128::from(us) * u128::from(self.fps.numerator())
            / (1_000_000 * u128::from(self.fps.denominator().max(1)));
        u64::try_from(value).unwrap_or(u64::MAX)
    }
}

fn span_us(span: &ReviewSpan) -> Range<u64> {
    span.start.floor()..span.end.ceil()
}

fn overlap(a: &Range<u64>, b: &Range<u64>) -> u64 {
    a.end.min(b.end).saturating_sub(a.start.max(b.start))
}

fn inside(point: u64, spans: &[Range<u64>]) -> bool {
    spans.iter().any(|span| span.contains(&point))
}

pub(crate) fn has_audio_segments(expectations: &ReviewExpectations) -> bool {
    expectations
        .segments
        .iter()
        .any(|segment| segment.id.lane == ReviewLane::Audio)
}

pub(crate) fn judge(
    summary: &ScanSummary,
    inputs: &ReviewScanInputs<'_>,
    config: &TechnicalScanConfig,
) -> TechnicalReviewReport {
    let mut report = unrun(config);
    report.loudness.result = ReviewRowResult::Pass;
    report.defects.result = ReviewRowResult::Pass;
    report.decoded_frames = u64::try_from(summary.video.frames.len()).unwrap_or(u64::MAX);
    report.decoded_audio_samples = summary
        .audio
        .as_ref()
        .map_or(0, |audio| audio.decoded_samples);
    report.measured = summary
        .audio
        .as_ref()
        .and_then(|audio| audio.loudness.as_ref().ok().copied());
    let clock = Clock::new(inputs.expectations.output_fps);
    structure(&mut report.structure, summary, inputs, config, &clock);
    loudness(&mut report.loudness, summary, inputs.expectations, config);
    let regions = Regions::new(inputs.expectations);
    picture_defects(
        &mut report.defects,
        &summary.video,
        &regions,
        config,
        &clock,
    );
    if let Some(audio) = &summary.audio {
        audio_defects(&mut report.defects, audio, &regions, config);
    }
    report
}

fn structure(
    row: &mut ReviewRowReport,
    summary: &ScanSummary,
    inputs: &ReviewScanInputs<'_>,
    config: &TechnicalScanConfig,
    clock: &Clock,
) {
    let expectations = inputs.expectations;
    let settings = inputs.settings;
    // The owned-input manifest, not the working file system, proves media
    // presence; every other document error stands.
    for issue in kinewright_core::qa_document(inputs.document).issues {
        if issue.severity == QaSeverity::Error && issue.code != "missing_media" {
            row.push(FindingKind::DocumentQa { code: issue.code }, Failed, [0, 0]);
        }
    }
    let mut setting = |code: &str| {
        row.push(
            FindingKind::DeliverySettings { code: code.into() },
            Failed,
            [0, 0],
        );
    };
    if settings.fps != expectations.output_fps {
        setting("fps");
    }
    if settings.resolution != expectations.raster {
        setting("resolution");
    }
    if !settings.resolution.0.is_multiple_of(2) || !settings.resolution.1.is_multiple_of(2) {
        setting("odd_raster");
    }
    if let Some(mismatch) = kinewright_core::delivery_color_mismatch(&settings.delivery_color) {
        setting(&format!("delivery_color_{}", mismatch.field));
    }
    let video = &summary.video;
    let end_us = clock.frame_us(u64::try_from(expectations.output_frames.0).unwrap_or(0));
    if (video.width, video.height) != expectations.raster {
        row.push(
            FindingKind::DeliverySettings {
                code: "decoded_raster".into(),
            },
            Failed,
            [0, end_us],
        );
    }
    match &video.color {
        Some(observed) => {
            let check = kinewright_core::delivery_tag_check(
                &settings.delivery_color,
                observed,
                DeliveryTagSource::ProbedOutputFile,
            );
            for mismatch in check.mismatches {
                row.push(
                    FindingKind::DeliveryTag {
                        field: mismatch.field,
                    },
                    Failed,
                    [0, end_us],
                );
            }
        }
        None => row.push(
            FindingKind::FrameCount {
                expected: u64::try_from(expectations.output_frames.0).unwrap_or(0),
                decoded: 0,
            },
            Failed,
            [0, end_us],
        ),
    }
    if video.too_long {
        row.push(FindingKind::TooLong, Undecided, [0, end_us]);
        return;
    }
    video_timing(row, video, inputs, clock, end_us);
    match &summary.audio {
        Some(audio) => audio_timing(row, audio, config, end_us),
        None if has_audio_segments(expectations) => {
            row.push(FindingKind::AudioStreamMissing, Failed, [0, end_us]);
        }
        None => {}
    }
    sync(row, summary, expectations, config, clock);
}

fn video_timing(
    row: &mut ReviewRowReport,
    video: &VideoSummary,
    inputs: &ReviewScanInputs<'_>,
    clock: &Clock,
    end_us: u64,
) {
    let timing = inputs.timing;
    let decoded = u64::try_from(video.frames.len()).unwrap_or(u64::MAX);
    if video.time_base != timing.video.time_base
        || video.first_pts != Some(timing.video_first_pts)
        || decoded != timing.video_frames
    {
        row.push(FindingKind::TimingRecordChanged, Undecided, [0, end_us]);
    }
    let Some(step) = super::decode::frame_step(video.time_base, inputs.expectations.output_fps)
    else {
        row.push(FindingKind::VideoFrameGrid, Failed, [0, end_us]);
        return;
    };
    if timing
        .video_pts_step
        .is_some_and(|recorded| recorded != step)
    {
        row.push(FindingKind::VideoFrameGrid, Failed, [0, end_us]);
    }
    if video.first_pts != Some(0) {
        row.push(
            FindingKind::VideoPts { frame: 0 },
            Failed,
            [0, clock.frame_us(1)],
        );
    }
    for &frame in &video.off_grid {
        row.push(
            FindingKind::VideoPts { frame },
            Failed,
            [clock.frame_us(frame), clock.frame_us(frame + 1)],
        );
    }
    let expected = u64::try_from(inputs.expectations.output_frames.0).unwrap_or(0);
    if decoded != expected {
        row.push(
            FindingKind::FrameCount { expected, decoded },
            Failed,
            [
                clock.frame_us(decoded.min(expected)),
                clock.frame_us(decoded.max(expected)),
            ],
        );
    }
}

fn samples_us(samples: i64, rate: u32) -> u64 {
    let value = i128::from(samples.max(0)) * 1_000_000 / i128::from(rate.max(1));
    u64::try_from(value).unwrap_or(u64::MAX)
}

fn audio_timing(
    row: &mut ReviewRowReport,
    audio: &AudioSummary,
    config: &TechnicalScanConfig,
    end_us: u64,
) {
    if audio.too_long {
        row.push(FindingKind::TooLong, Undecided, [0, end_us]);
        return;
    }
    for event in &audio.events {
        match *event {
            TimingEvent::Gap { at, samples } => row.push(
                FindingKind::AudioGap { samples },
                Failed,
                [
                    samples_us(at - i64::try_from(samples).unwrap_or(0), audio.rate),
                    samples_us(at, audio.rate),
                ],
            ),
            TimingEvent::Overlap { at, samples } => row.push(
                FindingKind::AudioOverlap { samples },
                Failed,
                [samples_us(at, audio.rate), samples_us(at, audio.rate)],
            ),
        }
    }
    let tolerance = i64::try_from(config.audio_extent_tolerance_samples).unwrap_or(i64::MAX);
    let extent =
        i64::try_from(u128::from(end_us) * u128::from(audio.rate) / 1_000_000).unwrap_or(i64::MAX);
    let Some(first) = audio.first else {
        row.push(
            FindingKind::AudioShort {
                samples: extent.unsigned_abs(),
            },
            Failed,
            [0, end_us],
        );
        return;
    };
    if first > tolerance {
        row.push(
            FindingKind::AudioStartLate {
                samples: first.unsigned_abs(),
            },
            Failed,
            [0, samples_us(first, audio.rate)],
        );
    }
    if audio.end < extent - tolerance {
        row.push(
            FindingKind::AudioShort {
                samples: (extent - audio.end).unsigned_abs(),
            },
            Failed,
            [samples_us(audio.end, audio.rate), end_us],
        );
    } else if audio.end > extent + tolerance {
        row.push(
            FindingKind::AudioLong {
                samples: (audio.end - extent).unsigned_abs(),
            },
            Failed,
            [end_us, samples_us(audio.end, audio.rate)],
        );
    }
}

/// Floor for level comparisons so digital zero does not dominate rises.
const LEVEL_FLOOR_CDB: i16 = -10_000;

fn level(audio: &AudioSummary, hop: usize) -> i16 {
    audio
        .hops
        .get(hop)
        .map_or(LEVEL_FLOOR_CDB, |stat| stat.rms_cdb.max(LEVEL_FLOOR_CDB))
}

/// Largest 10 ms rise over the preceding 50 ms near `at_us`.
fn audio_onset(audio: &AudioSummary, at_us: u64, config: &TechnicalScanConfig) -> Option<u64> {
    let first = usize::try_from(at_us.saturating_sub(config.sync_search_us) / SCAN_HOP_MICROS)
        .unwrap_or(usize::MAX);
    let last = usize::try_from((at_us + config.sync_search_us) / SCAN_HOP_MICROS)
        .unwrap_or(usize::MAX)
        .min(audio.hops.len().saturating_sub(1));
    let mut best: Option<(i32, usize)> = None;
    for hop in first.max(1)..=last {
        let before = (hop.saturating_sub(5)..hop)
            .map(|index| level(audio, index))
            .max()
            .unwrap_or(LEVEL_FLOOR_CDB);
        let rise = i32::from(level(audio, hop)) - i32::from(before);
        if rise >= i32::from(config.sync_onset_rise_cdb) && best.is_none_or(|(old, _)| rise > old) {
            best = Some((rise, hop));
        }
    }
    best.map(|(_, hop)| u64::try_from(hop).unwrap_or(u64::MAX) * SCAN_HOP_MICROS)
}

/// Largest frame-to-frame mean luma step near `at_us`.
fn visual_event(
    video: &VideoSummary,
    at_us: u64,
    config: &TechnicalScanConfig,
    clock: &Clock,
) -> Option<u64> {
    let first = clock
        .frame_at(at_us.saturating_sub(config.sync_search_us))
        .max(1);
    let last = clock.frame_at(at_us + config.sync_search_us).min(
        u64::try_from(video.frames.len())
            .unwrap_or(0)
            .saturating_sub(1),
    );
    let mut best: Option<(u8, u64)> = None;
    for frame in first..=last {
        let index = usize::try_from(frame).unwrap_or(usize::MAX);
        let step = video.frames[index]
            .mean
            .abs_diff(video.frames[index - 1].mean);
        if step >= config.sync_visual_step && best.is_none_or(|(old, _)| step > old) {
            best = Some((step, frame));
        }
    }
    best.map(|(_, frame)| clock.frame_us(frame))
}

fn sync(
    row: &mut ReviewRowReport,
    summary: &ScanSummary,
    expectations: &ReviewExpectations,
    config: &TechnicalScanConfig,
    clock: &Clock,
) {
    if !expectations.av_landmarks.is_determined() {
        row.push(FindingKind::LandmarksNotDetermined, Undecided, [0, 0]);
    }
    for (index, landmark) in expectations.av_landmarks.known().enumerate() {
        let landmark_id = u32::try_from(index).unwrap_or(u32::MAX);
        let audio_at = landmark.audio_at.floor();
        let video_at = landmark.video_at.floor();
        let span = [audio_at.min(video_at), audio_at.max(video_at)];
        let onset = summary
            .audio
            .as_ref()
            .and_then(|audio| audio_onset(audio, audio_at, config));
        let event = visual_event(&summary.video, video_at, config, clock);
        let (Some(onset), Some(event)) = (onset, event) else {
            row.push(
                FindingKind::SyncEventUndetected {
                    landmark: landmark_id,
                },
                Undecided,
                span,
            );
            continue;
        };
        let measured = i128::from(onset) - i128::from(event);
        let expected = i128::from(audio_at) - i128::from(video_at);
        let error = u64::try_from((measured - expected).unsigned_abs()).unwrap_or(u64::MAX);
        let allowed = landmark
            .uncertainty
            .ceil()
            .saturating_add(SCAN_HOP_MICROS)
            .saturating_add(clock.frame_us(1));
        if error > allowed {
            row.push(
                FindingKind::SyncOffset {
                    landmark: landmark_id,
                    error_us: error,
                    allowed_us: allowed,
                },
                Failed,
                span,
            );
        }
    }
}

fn loudness(
    row: &mut ReviewRowReport,
    summary: &ScanSummary,
    expectations: &ReviewExpectations,
    config: &TechnicalScanConfig,
) {
    let audible = has_audio_segments(expectations);
    let Some(audio) = &summary.audio else {
        if audible {
            row.push(FindingKind::LoudnessUnmeasured, Undecided, [0, 0]);
        }
        return;
    };
    let Ok(measured) = &audio.loudness else {
        row.push(FindingKind::LoudnessUnmeasured, Undecided, [0, 0]);
        return;
    };
    match measured.integrated_lufs_hundredths {
        Some(lufs) => {
            if (lufs - config.integrated_lufs_hundredths).abs() > config.tolerance_lu_hundredths {
                row.push(
                    FindingKind::IntegratedLoudness {
                        lufs_hundredths: lufs,
                    },
                    Failed,
                    [0, 0],
                );
            }
        }
        None if audible => row.push(FindingKind::LoudnessUnmeasured, Undecided, [0, 0]),
        None => {}
    }
    if let Some(peak) = measured.true_peak_dbtp_hundredths
        && peak > config.true_peak_ceiling_dbtp_hundredths
    {
        row.push(
            FindingKind::TruePeak {
                dbtp_hundredths: peak,
            },
            Failed,
            [0, 0],
        );
    }
}

/// Authored structure the defect scan judges against.
struct Regions {
    /// Output spans of video segments that sample a moving source.
    moving_video: Vec<Range<u64>>,
    /// Intended pauses, authored cards and their visible runs.
    authored: Vec<Range<u64>>,
    /// Video segment and overlay run edges: intended cuts.
    picture_edges: Vec<u64>,
    audio_segments: Vec<Range<u64>>,
    /// Audio segment edges, where only the join audit decides.
    audio_edges: Vec<u64>,
    intended_pauses: Vec<Range<u64>>,
    speech: Vec<Range<u64>>,
    speech_determined: bool,
}

impl Regions {
    fn new(expectations: &ReviewExpectations) -> Self {
        let lane = |lane: ReviewLane| {
            expectations
                .segments
                .iter()
                .filter(move |segment| segment.id.lane == lane)
        };
        let moving =
            |segment: &&ReviewSegment| matches!(segment.sampling, SegmentSampling::Video { .. });
        let intended_pauses: Vec<_> = expectations.intended_pauses.known().map(span_us).collect();
        let overlays = &expectations.overlays;
        let mut authored = intended_pauses.clone();
        let mut picture_edges = Vec::new();
        for segment in lane(ReviewLane::Video) {
            let span = span_us(&segment.output);
            picture_edges.extend([span.start, span.end]);
        }
        for overlay in overlays
            .captions
            .known()
            .chain(overlays.titles.known())
            .chain(overlays.cards.known())
        {
            for run in &overlay.visible {
                let span = span_us(&run.output);
                picture_edges.extend([span.start, span.end]);
            }
        }
        for card in overlays.cards.known() {
            authored.push(span_us(&card.cue));
            authored.extend(card.visible.iter().map(|run| span_us(&run.output)));
        }
        let mut audio_edges = Vec::new();
        let audio_segments: Vec<_> = lane(ReviewLane::Audio)
            .map(|segment| {
                let span = span_us(&segment.output);
                audio_edges.extend([span.start, span.end]);
                span
            })
            .collect();
        Self {
            moving_video: lane(ReviewLane::Video)
                .filter(moving)
                .map(|segment| span_us(&segment.output))
                .collect(),
            authored,
            picture_edges,
            audio_segments,
            audio_edges,
            intended_pauses,
            speech: expectations.speech_windows.known().map(span_us).collect(),
            speech_determined: expectations.speech_windows.is_determined(),
        }
    }

    /// The picture at `us` should be live moving source, so black, freeze
    /// and flash there are defects.
    fn judges_picture(&self, us: u64) -> bool {
        inside(us, &self.moving_video) && !inside(us, &self.authored)
    }

    fn near_picture_edge(&self, frame: u64, clock: &Clock) -> bool {
        self.picture_edges
            .iter()
            .any(|&edge| clock.frame_at(edge).abs_diff(frame) <= 1)
    }

    /// The whole hop sits in kept audio away from joins and intended pauses.
    fn judges_audio(&self, hop: Range<u64>, tolerance: u64) -> bool {
        self.audio_segments
            .iter()
            .any(|span| span.start <= hop.start && hop.end <= span.end)
            && !self
                .intended_pauses
                .iter()
                .any(|pause| overlap(pause, &hop) > 0)
            && !self
                .audio_edges
                .iter()
                .any(|&edge| edge + tolerance > hop.start && edge < hop.end + tolerance)
    }
}

fn picture_defects(
    row: &mut ReviewRowReport,
    video: &VideoSummary,
    regions: &Regions,
    config: &TechnicalScanConfig,
    clock: &Clock,
) {
    let frames = &video.frames;
    let count = u64::try_from(frames.len()).unwrap_or(u64::MAX);
    let judged = |frame: u64| regions.judges_picture(clock.frame_us(frame));
    let frame_stat = |frame: u64| frames[usize::try_from(frame).unwrap_or(usize::MAX)];
    // Black and freeze: runs of judged frames.
    let mut dark_from: Option<u64> = None;
    let mut frozen_from: Option<u64> = None;
    for frame in 0..=count {
        let live = frame < count && judged(frame);
        let is_black = live && {
            let measured = frame_stat(frame);
            measured.mean <= config.black_mean_max && measured.max_cell <= config.black_cell_max
        };
        match (dark_from, is_black) {
            (None, true) => dark_from = Some(frame),
            (Some(first), false) => {
                if frame - first >= u64::from(config.black_min_frames) {
                    row.push(
                        FindingKind::Black,
                        Failed,
                        [clock.frame_us(first), clock.frame_us(frame)],
                    );
                }
                dark_from = None;
            }
            _ => {}
        }
        // A repeat continues a freeze only when both frames are judged.
        let repeat = live
            && frame > 0
            && judged(frame - 1)
            && frame_stat(frame).difference <= config.freeze_max_difference;
        match (frozen_from, repeat) {
            (None, true) => frozen_from = Some(frame - 1),
            (Some(first), false) => {
                if clock.frame_us(frame) - clock.frame_us(first) >= config.freeze_min_us {
                    row.push(
                        FindingKind::Freeze,
                        Failed,
                        [clock.frame_us(first), clock.frame_us(frame)],
                    );
                }
                frozen_from = None;
            }
            _ => {}
        }
    }
    // Flash: a large step away and back within a few frames.
    let mut frame = 1;
    while frame < count {
        let before = frame_stat(frame - 1).mean;
        if frame_stat(frame).mean.abs_diff(before) < config.flash_step || !judged(frame) {
            frame += 1;
            continue;
        }
        let returned = (1..=u64::from(config.flash_max_frames))
            .map(|offset| frame + offset)
            .take_while(|&later| later < count)
            .find(|&later| frame_stat(later).mean.abs_diff(before) <= config.flash_return);
        match returned {
            Some(back_at) => {
                let authored_cut = regions.near_picture_edge(frame, clock)
                    && regions.near_picture_edge(back_at, clock);
                if !authored_cut {
                    row.push(
                        FindingKind::Flash,
                        Failed,
                        [clock.frame_us(frame), clock.frame_us(back_at)],
                    );
                }
                frame = back_at + 1;
            }
            None => frame += 1,
        }
    }
}

fn audio_defects(
    row: &mut ReviewRowReport,
    audio: &AudioSummary,
    regions: &Regions,
    config: &TechnicalScanConfig,
) {
    let hop_range = |hop: usize| {
        let start = u64::try_from(hop).unwrap_or(u64::MAX) * SCAN_HOP_MICROS;
        start..start + SCAN_HOP_MICROS
    };
    let judged = |hop: usize| regions.judges_audio(hop_range(hop), config.join_tolerance_us);
    for (hop, stat) in audio.hops.iter().enumerate() {
        if stat.click && judged(hop) {
            let span = hop_range(hop);
            row.push(FindingKind::Click, Failed, [span.start, span.end]);
        }
    }
    let silent = |hop: usize| {
        audio
            .hops
            .get(hop)
            .is_some_and(|stat| stat.rms_cdb <= config.silence_rms_cdb)
    };
    let mut hop = 0;
    while hop < audio.hops.len() {
        if !silent(hop) {
            hop += 1;
            continue;
        }
        let start = hop;
        while hop < audio.hops.len() && silent(hop) {
            hop += 1;
        }
        let run = hop_range(start).start..hop_range(hop).start;
        let length = run.end - run.start;
        if length <= config.dropout_max_us {
            let loud = |index: Option<usize>| {
                index.is_some_and(|index| {
                    index < audio.hops.len()
                        && i32::from(level(audio, index))
                            >= i32::from(config.silence_rms_cdb)
                                + i32::from(config.dropout_contrast_cdb)
                })
            };
            // A hole inside kept speech; outside known speech it is a pause
            // unless the speech inventory is incomplete.
            if loud(start.checked_sub(1)) && loud(Some(hop)) && (start..hop).all(judged) {
                if regions
                    .speech
                    .iter()
                    .any(|speech| overlap(speech, &run) == length)
                {
                    row.push(FindingKind::Dropout, Failed, [run.start, run.end]);
                } else if !regions.speech_determined {
                    row.push(FindingKind::Dropout, Undecided, [run.start, run.end]);
                }
            }
        }
        if length >= config.silence_gap_min_us {
            silence_gap(row, &run, regions, config);
        }
    }
}

fn silence_gap(
    row: &mut ReviewRowReport,
    run: &Range<u64>,
    regions: &Regions,
    config: &TechnicalScanConfig,
) {
    let kept = regions
        .audio_segments
        .iter()
        .map(|segment| overlap(segment, run))
        .sum::<u64>();
    let paused = regions
        .intended_pauses
        .iter()
        .map(|pause| overlap(pause, run))
        .sum::<u64>();
    let unpaused_speech = regions
        .speech
        .iter()
        .map(|speech| {
            let shared = speech.start.max(run.start)..speech.end.min(run.end);
            if shared.start >= shared.end {
                return 0;
            }
            let length = shared.end - shared.start;
            let paused = regions
                .intended_pauses
                .iter()
                .map(|pause| overlap(pause, &shared))
                .sum::<u64>();
            length.saturating_sub(paused)
        })
        .sum::<u64>();
    if unpaused_speech >= config.silence_over_speech_min_us {
        row.push(FindingKind::SilenceOverSpeech, Failed, [run.start, run.end]);
    } else if !regions.speech_determined
        && kept.saturating_sub(paused) >= config.silence_over_speech_min_us
    {
        row.push(
            FindingKind::SilenceWithUndeterminedSpeech,
            Undecided,
            [run.start, run.end],
        );
    }
}
