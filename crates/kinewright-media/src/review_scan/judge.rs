//! Decision rules. Pure: the decoded per-frame and per-hop statistics, the
//! plan and the frozen settings in; four row reports out.
use std::ops::Range;

use kinewright_core::output_review::RequiredReviewRow;
use kinewright_core::{DeliveryTagSource, MediaError, QaSeverity};

use super::picture::{FrameState, PictureScan};
use super::plan::{FrameCursor, Plan, samples_at};
use super::sound::{OutputSound, SoundScan, SourceHop, TimingEvent, first_hop, linear};
use super::speech::{JoinScan, JoinVerdict};
use super::{
    CoverageUnit, ExcludedSpan, ExclusionReason, FindingKind, FindingVerdict, MAX_EXCLUDED_SPANS,
    MAX_SYNC_WINDOWS, RetainedSpeechConfig, ReviewCheckReport, ReviewRowReport, ReviewScanInputs,
    SCAN_HOP_MICROS, ScanPass, SyncMeasurement, SyncSignal, TechnicalScanConfig,
};

use FindingVerdict::{Failed, Undecided};

/// Level floor so digital zero does not dominate dB arithmetic.
const FLOOR_DB: f64 = -100.0;
/// A part dominates a hop when it holds this share of the predicted power.
const DOMINANT_SHARE: f64 = 0.9;
/// Sync: a window is measurable when this share of its hops is predicted...
const SYNC_COVERED_SHARE: f64 = 0.8;
/// ...its predicted envelope varies by at least this many dB...
const SYNC_ENVELOPE_STD_DB: f64 = 3.0;
/// ...and no other lag further than two steps away scores within this.
const SYNC_DISTINCT: f64 = 0.05;
/// Static source frames in a row that end a freeze run.
const FREEZE_STATIC_RESET: usize = 2;
/// Least compared frame pairs for a video lag.
const SYNC_MIN_PAIRS: usize = 8;

fn db(power: f64) -> f64 {
    if power <= 0.0 {
        FLOOR_DB
    } else {
        (10.0 * power.log10()).max(FLOOR_DB)
    }
}

fn power(cdb: i16) -> f64 {
    let amplitude = linear(cdb);
    amplitude * amplitude
}

fn median(values: &mut [f64]) -> Option<f64> {
    if values.is_empty() {
        return None;
    }
    values.sort_by(f64::total_cmp);
    Some(values[values.len() / 2])
}

fn hop_us(hop: u64) -> u64 {
    hop.saturating_mul(SCAN_HOP_MICROS)
}

pub(super) fn judge(
    inputs: &ReviewScanInputs<'_>,
    plan: &Plan<'_>,
    picture: &Result<PictureScan, MediaError>,
    sound: &Result<SoundScan, MediaError>,
    joins: &Result<JoinScan, MediaError>,
    technical: &TechnicalScanConfig,
    speech: &RetainedSpeechConfig,
) -> ReviewCheckReport {
    let picture = picture.as_ref();
    let sound = sound.as_ref();
    let output = sound.ok().and_then(|scan| scan.output.as_ref());
    let model = match (sound, output) {
        (Ok(scan), Some(output)) if !output.truncated && !scan.sources.truncated => {
            Some(Model::new(
                plan,
                scan,
                output,
                technical,
                inputs.settings.loudness_normalization.is_some(),
            ))
        }
        _ => None,
    };
    let end_us = plan.clock.frame_us(plan.output_frames);
    let early = output.is_some_and(|audio| {
        has_expected_audio(plan) && audio_ends_early(inputs, plan, audio, technical, end_us)
    });
    let mut report = ReviewCheckReport {
        technical_sha256: technical.sha256(),
        speech_sha256: speech.sha256(),
        structure: ReviewRowReport::new(RequiredReviewRow::StructureAndAvIntegrity),
        loudness: ReviewRowReport::new(RequiredReviewRow::LoudnessAndPeak),
        defects: ReviewRowReport::new(RequiredReviewRow::TechnicalDefectScan),
        speech: ReviewRowReport::new(RequiredReviewRow::RetainedSpeechChecks),
        excluded: Vec::new(),
        excluded_omitted: 0,
        decoded_frames: picture.map_or(0, |scan| u64::try_from(scan.frames.len()).unwrap_or(0)),
        decoded_audio_samples: output.map_or(0, |output| output.decoded_samples),
        measured: output.and_then(|output| output.loudness.as_ref().ok().copied()),
        sync: SyncMeasurement::default(),
    };
    structure(
        &mut report.structure,
        inputs,
        plan,
        picture,
        sound,
        technical,
        end_us,
    );
    let (sync, sync_findings) = sync(plan, picture.ok(), model.as_ref(), technical);
    report.sync = sync;
    for (kind, verdict) in sync_findings {
        report.structure.push(kind, verdict, [0, end_us]);
    }
    loudness(
        &mut report.loudness,
        inputs,
        plan,
        sound,
        technical,
        end_us,
        early,
    );
    edit_defects(&mut report.defects, inputs, plan, technical);
    let (excluded, omitted) =
        picture_defects(&mut report.defects, plan, picture, technical, end_us);
    report.excluded = excluded;
    report.excluded_omitted = omitted;
    sound_defects(
        &mut report.defects,
        plan,
        sound,
        model.as_ref(),
        technical,
        end_us,
        early,
    );
    retained_speech(
        &mut report.speech,
        inputs,
        plan,
        sound,
        model.as_ref(),
        speech,
    );
    joins_row(&mut report.speech, joins);
    report
}

fn has_expected_audio(plan: &Plan<'_>) -> bool {
    plan.audio.iter().any(|part| !part.muted)
}

#[allow(clippy::too_many_lines, reason = "one rule per container property")]
fn structure(
    row: &mut ReviewRowReport,
    inputs: &ReviewScanInputs<'_>,
    plan: &Plan<'_>,
    picture: Result<&PictureScan, &MediaError>,
    sound: Result<&SoundScan, &MediaError>,
    config: &TechnicalScanConfig,
    end_us: u64,
) {
    let expectations = inputs.expectations;
    let settings = inputs.settings;
    let timing = inputs.timing;
    // The owned-input manifest, not the working file system, proves media
    // presence; every other document error stands.
    for issue in kinewright_core::qa_document(inputs.document).issues {
        if issue.severity == QaSeverity::Error && issue.code != "missing_media" {
            row.push(FindingKind::DocumentQa { code: issue.code }, Failed, [0, 0]);
        }
    }
    let setting = |row: &mut ReviewRowReport, code: &str| {
        row.push(
            FindingKind::DeliverySettings { code: code.into() },
            Failed,
            [0, 0],
        );
    };
    if settings.fps != expectations.output_fps {
        setting(row, "fps");
    }
    if settings.resolution != expectations.raster {
        setting(row, "resolution");
    }
    if !settings.resolution.0.is_multiple_of(2) || !settings.resolution.1.is_multiple_of(2) {
        setting(row, "odd_raster");
    }
    if let Some(mismatch) = kinewright_core::delivery_color_mismatch(&settings.delivery_color) {
        setting(row, &format!("delivery_color_{}", mismatch.field));
    }
    if timing.decoder_version != ffmpeg_next::codec::version() {
        row.push(FindingKind::TimingRecordChanged, Undecided, [0, end_us]);
    }
    let clock = plan.clock;
    let expected_frames = plan.output_frames;
    match picture {
        Err(error) => row.push(
            FindingKind::Unreadable {
                pass: ScanPass::Picture,
                reason: super::reason(error),
            },
            Undecided,
            [0, end_us],
        ),
        Ok(video) => {
            let decoded = u64::try_from(video.frames.len()).unwrap_or(u64::MAX);
            row.cover(
                CoverageUnit::Frames,
                decoded.min(expected_frames),
                0,
                expected_frames,
            );
            if video.video_streams > 1 {
                row.push(
                    FindingKind::ExtraVideoStream {
                        streams: video.video_streams,
                    },
                    Failed,
                    [0, end_us],
                );
            }
            if video.size != expectations.raster {
                setting(row, "decoded_raster");
            }
            if let Some(observed) = &video.color {
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
            if video.time_base != timing.video.time_base
                || video.decoder != timing.video.decoder
                || video.first_pts != Some(timing.video_first_pts)
                || (!video.truncated && decoded != timing.video_frames)
            {
                row.push(FindingKind::TimingRecordChanged, Undecided, [0, end_us]);
            }
            if video.truncated {
                row.push(
                    FindingKind::ScanTruncated {
                        pass: ScanPass::Picture,
                    },
                    Undecided,
                    [clock.frame_us(decoded), end_us],
                );
            }
            match super::reader::frame_step(video.time_base, expectations.output_fps) {
                Some(step)
                    if timing
                        .video_pts_step
                        .is_none_or(|recorded| recorded == step) => {}
                _ => row.push(FindingKind::VideoFrameGrid, Failed, [0, end_us]),
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
            row.omitted += video
                .off_grid_count
                .saturating_sub(u64::try_from(video.off_grid.len()).unwrap_or(0));
            if !video.truncated && decoded != expected_frames {
                row.push(
                    FindingKind::FrameCount {
                        expected: expected_frames,
                        decoded,
                    },
                    Failed,
                    [
                        clock.frame_us(decoded.min(expected_frames)),
                        clock.frame_us(decoded.max(expected_frames)),
                    ],
                );
            }
        }
    }
    let expected_audio = has_expected_audio(plan);
    let scan = match sound {
        Err(error) => {
            row.push(
                FindingKind::Unreadable {
                    pass: ScanPass::Sound,
                    reason: super::reason(error),
                },
                Undecided,
                [0, end_us],
            );
            return;
        }
        Ok(scan) => scan,
    };
    let recorded = u32::from(timing.audio.is_some());
    if scan.streams > recorded {
        row.push(
            FindingKind::ExtraAudioStream {
                streams: scan.streams,
            },
            Failed,
            [0, end_us],
        );
    }
    let Some(audio) = &scan.output else {
        if expected_audio {
            row.push(FindingKind::AudioStreamMissing, Failed, [0, end_us]);
        }
        return;
    };
    let Some(stream) = &timing.audio else {
        return;
    };
    if audio.time_base != stream.time_base || audio.decoder != stream.decoder {
        row.push(FindingKind::TimingRecordChanged, Undecided, [0, end_us]);
    }
    let silent = config.silent_dbfs;
    if !expected_audio
        && audio.hops.iter().any(|hop| {
            hop.rms
                .iter()
                .any(|&level| db(power(level)) > f64::from(silent))
        })
    {
        row.push(FindingKind::UnexpectedAudio, Failed, [0, end_us]);
    }
    audio_timing(row, audio, stream, plan, config, end_us);
}

fn samples_us(samples: i64, rate: u32) -> u64 {
    let value = i128::from(samples.max(0)) * 1_000_000 / i128::from(rate.max(1));
    u64::try_from(value).unwrap_or(u64::MAX)
}

/// Most priming an encoder plausibly places before output sample zero, in
/// coded frames (AAC primes 1024 to 2112 samples).
const PRIMING_MAX_FRAMES: u64 = 4;

/// Priming the encoder placed before output sample zero, from the recorded
/// stream start (an MP4 edit list starts the stream at minus the priming).
/// `None` when the start is missing (`AV_NOPTS_VALUE`) or implausible.
fn priming_samples(stream: &crate::ReviewOutputStream, rate: u32, frame_size: u32) -> Option<u64> {
    if stream.start_pts == i64::MIN {
        return None;
    }
    let (numerator, denominator) = stream.time_base;
    let samples = i128::from(stream.start_pts.min(0))
        .unsigned_abs()
        .saturating_mul(u128::from(numerator.unsigned_abs()))
        .saturating_mul(u128::from(rate))
        / u128::from(denominator.unsigned_abs().max(1));
    let bound = PRIMING_MAX_FRAMES.saturating_mul(u64::from(frame_size.max(1024)));
    u64::try_from(samples)
        .ok()
        .filter(|samples| *samples <= bound)
}

/// Output samples the cut spans at `rate`.
fn sample_extent(end_us: u64, rate: u32) -> i64 {
    i64::try_from(u128::from(end_us) * u128::from(rate) / 1_000_000).unwrap_or(i64::MAX)
}

/// How far the decoded audio end may fall from the cut's end: the last
/// coded frame is whole, the priming shifts it, and an fps change can round
/// the cut to one output frame.
fn end_tolerance(
    audio: &OutputSound,
    plan: &Plan<'_>,
    config: &TechnicalScanConfig,
    priming: u64,
) -> i64 {
    let rate = audio.rate;
    let frame_samples = u64::from(rate) * u64::from(plan.clock.fps().denominator())
        / u64::from(plan.clock.fps().numerator().max(1));
    i64::try_from(
        u64::from(audio.frame_size.max(1))
            .saturating_add(priming)
            .saturating_add(frame_samples)
            .saturating_add(config.audio_pts_tolerance_samples),
    )
    .unwrap_or(i64::MAX)
}

/// R2: the decoded audio stops (or never starts) before the cut ends, so
/// the sound and loudness checks saw only part of it. A missing start uses
/// no priming, the stricter end.
fn audio_ends_early(
    inputs: &ReviewScanInputs<'_>,
    plan: &Plan<'_>,
    audio: &OutputSound,
    config: &TechnicalScanConfig,
    end_us: u64,
) -> bool {
    let priming = inputs
        .timing
        .audio
        .as_ref()
        .and_then(|stream| priming_samples(stream, audio.rate, audio.frame_size))
        .unwrap_or(0);
    let extent = sample_extent(end_us, audio.rate);
    let decoded = if audio.first.is_some() { audio.end } else { 0 };
    extent > 0 && decoded < extent.saturating_sub(end_tolerance(audio, plan, config, priming))
}

fn audio_timing(
    row: &mut ReviewRowReport,
    audio: &OutputSound,
    stream: &crate::ReviewOutputStream,
    plan: &Plan<'_>,
    config: &TechnicalScanConfig,
    end_us: u64,
) {
    let rate = audio.rate;
    let extent = sample_extent(end_us, rate);
    row.cover(
        CoverageUnit::Samples,
        u64::try_from(audio.end.clamp(0, extent)).unwrap_or(0),
        0,
        u64::try_from(extent).unwrap_or(0),
    );
    if audio.truncated {
        row.push(
            FindingKind::ScanTruncated {
                pass: ScanPass::Sound,
            },
            Undecided,
            [samples_us(audio.end, rate), end_us],
        );
    }
    if audio.missing_pts {
        row.push(FindingKind::AudioPtsMissing, Failed, [0, end_us]);
    }
    for event in &audio.events {
        match *event {
            TimingEvent::Gap { at, samples } => row.push(
                FindingKind::AudioGap { samples },
                Failed,
                [
                    samples_us(at.saturating_sub(i64::try_from(samples).unwrap_or(0)), rate),
                    samples_us(at, rate),
                ],
            ),
            TimingEvent::Overlap { at, samples } => row.push(
                FindingKind::AudioOverlap { samples },
                Failed,
                [samples_us(at, rate), samples_us(at, rate)],
            ),
        }
    }
    row.omitted += audio
        .event_count
        .saturating_sub(u64::try_from(audio.events.len()).unwrap_or(0));
    let tolerance = config.audio_pts_tolerance_samples;
    if audio.early_samples > tolerance {
        // Decoded audio before the first picture is never priming: the
        // decoder drops what the edit list marks.
        row.push(
            FindingKind::AudioEarly {
                samples: audio.early_samples,
            },
            Failed,
            [0, 0],
        );
    }
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
    if first > i64::try_from(tolerance).unwrap_or(i64::MAX) {
        row.push(
            FindingKind::AudioStartLate {
                samples: first.unsigned_abs(),
            },
            Failed,
            [0, samples_us(first, rate)],
        );
    }
    if audio.truncated {
        return;
    }
    let Some(priming) = priming_samples(stream, rate, audio.frame_size) else {
        row.push(FindingKind::AudioStartUnknown, Undecided, [0, end_us]);
        return;
    };
    let end_tolerance = end_tolerance(audio, plan, config, priming);
    if audio.end < extent.saturating_sub(end_tolerance) {
        row.push(
            FindingKind::AudioShort {
                samples: extent.saturating_sub(audio.end).unsigned_abs(),
            },
            Failed,
            [samples_us(audio.end, rate), end_us],
        );
    } else if audio.end > extent.saturating_add(end_tolerance) {
        row.push(
            FindingKind::AudioLong {
                samples: audio.end.saturating_sub(extent).unsigned_abs(),
            },
            Failed,
            [end_us, samples_us(audio.end, rate)],
        );
    }
}

fn loudness(
    row: &mut ReviewRowReport,
    inputs: &ReviewScanInputs<'_>,
    plan: &Plan<'_>,
    sound: Result<&SoundScan, &MediaError>,
    config: &TechnicalScanConfig,
    end_us: u64,
    early: bool,
) {
    let expected = has_expected_audio(plan);
    let span = [0, end_us];
    // JA-1 S6 R1: loudness judged against no target is never a pass, with
    // or without audio, so the cut cannot seal under either label.
    if inputs.settings.loudness_normalization.is_none() {
        row.push(FindingKind::LoudnessUnmeasured, Undecided, span);
    }
    let Ok(Some(audio)) = sound.map(|decoded| decoded.output.as_ref()) else {
        // No scan, or no audio stream where audio is expected.
        if sound.is_err() || expected {
            row.push(FindingKind::LoudnessUnmeasured, Undecided, span);
        }
        return;
    };
    let extent =
        u64::try_from(u128::from(end_us) * u128::from(audio.rate) / 1_000_000).unwrap_or(u64::MAX);
    row.cover(
        CoverageUnit::Samples,
        u64::try_from(audio.end.max(0)).unwrap_or(0).min(extent),
        0,
        extent,
    );
    if audio.truncated {
        row.push(
            FindingKind::ScanTruncated {
                pass: ScanPass::Sound,
            },
            Undecided,
            span,
        );
        return;
    }
    let Ok(measured) = &audio.loudness else {
        row.push(FindingKind::LoudnessUnmeasured, Undecided, span);
        return;
    };
    // R2: loudness of part of the cut is not the cut's loudness.
    if early {
        row.push(FindingKind::LoudnessUnmeasured, Undecided, span);
    }
    match &inputs.settings.loudness_normalization {
        Some(target) => {
            match measured.integrated_lufs_hundredths {
                Some(lufs)
                    if (lufs - target.integrated_lufs_hundredths).abs()
                        > target.tolerance_lu_hundredths =>
                {
                    row.push(
                        FindingKind::IntegratedLoudness {
                            lufs_hundredths: lufs,
                            target_hundredths: target.integrated_lufs_hundredths,
                        },
                        Failed,
                        span,
                    );
                }
                None if expected => row.push(FindingKind::LoudnessUnmeasured, Undecided, span),
                Some(_) | None => {}
            }
            peak(
                row,
                measured,
                target.true_peak_ceiling_dbtp_hundredths,
                span,
            );
            if let (Some(max), Some(range)) = (
                target.loudness_range_max_lu_hundredths,
                measured.loudness_range_lu_hundredths,
            ) && range > max
            {
                row.push(
                    FindingKind::LoudnessRange {
                        lu_hundredths: range,
                        max_hundredths: max,
                    },
                    Failed,
                    span,
                );
            }
        }
        // No target: integrated loudness is recorded in `measured` only
        // (the row is already undecided above); the peak still must not clip.
        None => peak(
            row,
            measured,
            config.unnormalized_true_peak_ceiling_dbtp_hundredths,
            span,
        ),
    }
}

fn peak(
    row: &mut ReviewRowReport,
    measured: &kinewright_core::AudioLoudness,
    ceiling: i32,
    span: [u64; 2],
) {
    if let Some(peak) = measured.true_peak_dbtp_hundredths
        && peak > ceiling
    {
        row.push(
            FindingKind::TruePeak {
                dbtp_hundredths: peak,
                ceiling_hundredths: ceiling,
            },
            Failed,
            span,
        );
    }
}

/// Edit-level defects, exact from the plan: flash frames between two longer
/// runs of one source, frames no video segment covers, and kept words whose
/// audio is muted or missing.
fn edit_defects(
    row: &mut ReviewRowReport,
    inputs: &ReviewScanInputs<'_>,
    plan: &Plan<'_>,
    config: &TechnicalScanConfig,
) {
    let flash = u64::from(config.flash_max_frames);
    let clock = plan.clock;
    let mut cursor = FrameCursor::new(plan);
    // Runs of the topmost asset: (asset, first frame, length).
    let mut runs: Vec<(Option<kinewright_core::AssetId>, u64, u64)> = Vec::new();
    let mut hole: Option<u64> = None;
    let check_flash = |runs: &[(Option<kinewright_core::AssetId>, u64, u64)],
                       row: &mut ReviewRowReport| {
        // The middle of the last three runs.
        let [before, middle, after] = runs else {
            return;
        };
        if middle.0.is_some()
            && before.0.is_some()
            && before.0 == after.0
            && middle.0 != before.0
            && middle.2 <= flash
            && before.2 > flash
            && after.2 > flash
        {
            row.push(
                FindingKind::EditFlash { frames: middle.2 },
                Failed,
                [
                    clock.frame_us(middle.1),
                    clock.frame_us(middle.1 + middle.2),
                ],
            );
        }
    };
    let determined = plan.output_frames == 0 || plan.pictured(0).is_some();
    if !determined {
        row.push(
            FindingKind::TimelineUndetermined,
            Undecided,
            [0, clock.frame_us(plan.output_frames)],
        );
    }
    for frame in 0..plan.output_frames {
        cursor.advance(frame);
        // A hole is a frame no document clip draws, read from the frozen
        // document rather than from the mapped media segments.
        let empty = plan.pictured(frame) == Some(false);
        match (empty, hole) {
            (true, None) => hole = Some(frame),
            (false, Some(start)) => {
                row.push(
                    FindingKind::TimelineHole,
                    Failed,
                    [clock.frame_us(start), clock.frame_us(frame)],
                );
                hole = None;
            }
            _ => {}
        }
        let top = plan.top_asset(&cursor);
        match runs.last_mut() {
            Some(last) if last.0 == top => last.2 += 1,
            _ => {
                if runs.len() == 3 {
                    check_flash(&runs, row);
                    runs.remove(0);
                }
                runs.push((top, frame, 1));
            }
        }
    }
    if runs.len() == 3 {
        check_flash(&runs, row);
    }
    if let Some(start) = hole {
        row.push(
            FindingKind::TimelineHole,
            Failed,
            [clock.frame_us(start), clock.frame_us(plan.output_frames)],
        );
    }
    for word in inputs.expectations.kept_words.known() {
        let silent = plan
            .audio_part(&word.segment)
            .is_none_or(|part| plan.audio[part].muted);
        if silent {
            row.push(
                FindingKind::SilenceOverSpeech {
                    word_index: word.word_index,
                },
                Failed,
                [word.output.start.floor(), word.output.end.ceil()],
            );
        }
    }
}

/// Runs of consecutive frames satisfying a predicate, as frame ranges.
fn runs(frames: usize, mut test: impl FnMut(usize) -> bool) -> Vec<Range<u64>> {
    let mut found = Vec::new();
    let mut start = None;
    for index in 0..=frames {
        let hit = index < frames && test(index);
        match (hit, start) {
            (true, None) => start = Some(index),
            (false, Some(first)) => {
                found.push(u64::try_from(first).unwrap_or(0)..u64::try_from(index).unwrap_or(0));
                start = None;
            }
            _ => {}
        }
    }
    found
}

/// Freeze: within one part's compared run, the output holds still while
/// the mapped source moves. A static source frame neither counts nor
/// breaks the run, but more than `FREEZE_STATIC_RESET` of them in a row end
/// it (R7): a still source between two moving stretches is not one freeze.
/// A run counts at least `freeze_min_frames` moving frames; `eligible` is a
/// compared, non-black output frame. The caller applies `freeze_min_us`.
fn freeze_runs(
    frames: &[super::picture::FrameRecord],
    eligible: impl Fn(usize) -> bool,
    config: &TechnicalScanConfig,
) -> Vec<Range<usize>> {
    let mut found = Vec::new();
    let mut frozen = 0_u32;
    let mut still = 0_usize;
    let mut start: Option<usize> = None;
    let mut flush = |frozen: &mut u32, start: &mut Option<usize>, end: usize| {
        if *frozen >= config.freeze_min_frames
            && let Some(first) = *start
        {
            found.push(first..end);
        }
        *frozen = 0;
        *start = None;
    };
    for (index, frame) in frames.iter().enumerate() {
        let (Some(src), Some(out)) = (frame.src_diff, frame.out_diff) else {
            flush(&mut frozen, &mut start, index);
            still = 0;
            continue;
        };
        // A black output frame is reported as black, not also as a freeze.
        if !eligible(index) {
            flush(&mut frozen, &mut start, index);
            still = 0;
            continue;
        }
        if src < config.motion_floor {
            still += 1;
            if still > FREEZE_STATIC_RESET {
                flush(&mut frozen, &mut start, index + 1 - still);
            }
            continue;
        }
        still = 0;
        if out <= config.freeze_ratio * src {
            frozen += 1;
            start.get_or_insert(index);
        } else {
            flush(&mut frozen, &mut start, index);
        }
    }
    flush(&mut frozen, &mut start, frames.len());
    found
}

#[allow(clippy::too_many_lines, reason = "one rule per defect family")]
fn picture_defects(
    row: &mut ReviewRowReport,
    plan: &Plan<'_>,
    picture: Result<&PictureScan, &MediaError>,
    config: &TechnicalScanConfig,
    end_us: u64,
) -> (Vec<ExcludedSpan>, u64) {
    let clock = plan.clock;
    let span = |range: &Range<u64>| [clock.frame_us(range.start), clock.frame_us(range.end)];
    let video = match picture {
        Err(error) => {
            row.push(
                FindingKind::Unreadable {
                    pass: ScanPass::Picture,
                    reason: super::reason(error),
                },
                Undecided,
                [0, end_us],
            );
            return (Vec::new(), 0);
        }
        Ok(video) => video,
    };
    let frames = &video.frames;
    let count = frames.len();
    let compared = |index: usize| frames[index].state == FrameState::Compared;
    let is_black = |index: usize| {
        let frame = &frames[index];
        compared(index)
            && frame.out.mean <= config.black_mean_max
            && frame.out.max <= config.black_cell_max
            && (frame.src.mean > config.black_source_mean_min
                || frame.src.max > config.black_source_cell_min)
    };
    let textured = |std: f32| std >= 4.0 * config.texture_min;
    let flat = |std: f32| std < config.texture_min;
    let mismatched = |index: usize| {
        let frame = &frames[index];
        compared(index)
            && !is_black(index)
            && (frame.mad > config.mismatch_difference_max
                || frame
                    .corr
                    .is_some_and(|corr| corr < config.mismatch_correlation_min)
                || (textured(frame.out.std) && flat(frame.src.std))
                || (flat(frame.out.std) && textured(frame.src.std)))
    };
    for range in runs(count, is_black) {
        row.push(FindingKind::Black, Failed, span(&range));
    }
    // F3: a short mismatch is a flash wherever it falls, cuts included.
    for range in runs(count, mismatched) {
        let kind = if range.end - range.start <= u64::from(config.flash_max_frames) {
            FindingKind::Flash
        } else {
            FindingKind::PictureMismatch
        };
        row.push(kind, Failed, span(&range));
    }
    let eligible = |index: usize| compared(index) && !is_black(index);
    for range in freeze_runs(frames, eligible, config) {
        let range = u64::try_from(range.start).unwrap_or(0)..u64::try_from(range.end).unwrap_or(0);
        let [from, to] = span(&range);
        if to.saturating_sub(from) >= config.freeze_min_us {
            row.push(FindingKind::Freeze, Failed, [from, to]);
        }
    }
    // Not compared, by reason.
    for (reason_index, reason) in video.reasons.iter().enumerate() {
        let wanted = u16::try_from(reason_index).unwrap_or(u16::MAX);
        for range in runs(count, |index| {
            frames[index].state == FrameState::NotCompared(wanted)
        }) {
            row.push(
                FindingKind::PictureNotCompared {
                    reason: reason.clone(),
                },
                Undecided,
                span(&range),
            );
        }
    }
    let checked = frames
        .iter()
        .filter(|frame| matches!(frame.state, FrameState::Compared | FrameState::Empty))
        .count();
    let delegated = frames
        .iter()
        .filter(|frame| matches!(frame.state, FrameState::Excluded(_)))
        .count();
    let checked = u64::try_from(checked).unwrap_or(0);
    let delegated = u64::try_from(delegated).unwrap_or(0);
    let decoded = u64::try_from(count).unwrap_or(0);
    row.cover(CoverageUnit::Frames, checked, delegated, plan.output_frames);
    if video.truncated {
        row.push(
            FindingKind::ScanTruncated {
                pass: ScanPass::Picture,
            },
            Undecided,
            [clock.frame_us(decoded), end_us],
        );
    } else if decoded < plan.output_frames {
        row.push(
            FindingKind::PictureNotCompared {
                reason: "frames missing from the encode".into(),
            },
            Undecided,
            [clock.frame_us(decoded), end_us],
        );
    }
    // Spans the overlay check owns, merged per reason.
    let mut excluded = Vec::new();
    let mut omitted = 0_u64;
    for reason in [
        ExclusionReason::Composited,
        ExclusionReason::Effect,
        ExclusionReason::Transition,
        ExclusionReason::OverlayUnresolved,
        ExclusionReason::OverlayCovered,
        ExclusionReason::Generated,
    ] {
        for range in runs(count, |index| {
            frames[index].state == FrameState::Excluded(reason)
        }) {
            if excluded.len() < MAX_EXCLUDED_SPANS {
                excluded.push(ExcludedSpan {
                    reason,
                    output_us: span(&range),
                });
            } else {
                omitted += 1;
            }
        }
    }
    (excluded, omitted)
}

/// The mapped sources' predicted output levels on the hop clock, with each
/// part's gain measured from the encode where that part dominates.
struct Model<'s> {
    channels: usize,
    hops: usize,
    /// Output hops lag the source by this many hops.
    lag: i64,
    parts: Vec<Option<Range<usize>>>,
    entries: &'s [SourceHop],
    /// Measured dB gain per part and channel, over its static mix gain.
    gain: Vec<[f64; 2]>,
    static_db: &'s [[f64; 2]],
    /// Hops a muted-free but uncomparable or undecoded part covers.
    blocked: Vec<bool>,
    /// Hops an unmuted part covers.
    active: Vec<bool>,
    /// Hops at a non-continued edge of an unmuted part, ±1.
    edges: Vec<bool>,
    unity: [Vec<f32>; 2],
    predicted: [Vec<f32>; 2],
    second: [Vec<f32>; 2],
    output: &'s OutputSound,
    /// R3: parts whose measured gain is off the expected gain by more than
    /// `gain_tolerance_db`: (part, channel, measured minus expected dB).
    gain_changes: Vec<(usize, usize, f64)>,
}

fn part_hops(plan: &Plan<'_>, part: usize, rate: u32) -> Range<usize> {
    let segment = plan.audio[part].segment;
    let start = first_hop(samples_at(segment.output.start, rate), rate);
    let end = first_hop(samples_at(segment.output.end, rate).saturating_sub(1), rate) + 1;
    usize::try_from(start).unwrap_or(usize::MAX)..usize::try_from(end).unwrap_or(usize::MAX)
}

impl<'s> Model<'s> {
    fn new(
        plan: &Plan<'_>,
        scan: &'s SoundScan,
        output: &'s OutputSound,
        config: &TechnicalScanConfig,
        normalized: bool,
    ) -> Self {
        let channels = usize::from(output.channels).min(2);
        let hops = output.hops.len();
        let entries = scan.sources.hops.as_slice();
        let mut parts: Vec<Option<Range<usize>>> = vec![None; plan.audio.len()];
        let mut index = 0;
        while index < entries.len() {
            let part = usize::try_from(entries[index].part).unwrap_or(usize::MAX);
            let start = index;
            while index < entries.len() && entries[index].part == entries[start].part {
                index += 1;
            }
            if let Some(slot) = parts.get_mut(part) {
                *slot = Some(start..index);
            }
        }
        let mut blocked = vec![false; hops];
        let mut active = vec![false; hops];
        let mut edges = vec![false; hops];
        let mark = |flags: &mut Vec<bool>, range: Range<usize>| {
            for flag in flags.iter_mut().take(range.end).skip(range.start) {
                *flag = true;
            }
        };
        let failed: Vec<usize> = scan.sources.failed.iter().map(|(part, _)| *part).collect();
        for (part, audio) in plan.audio.iter().enumerate() {
            if audio.muted {
                continue;
            }
            let range = part_hops(plan, part, output.rate);
            mark(&mut active, range.clone());
            if audio.uncomparable.is_some() || failed.contains(&part) {
                mark(&mut blocked, range.clone());
            }
            for (side, at) in [range.start, range.end.saturating_sub(1)]
                .into_iter()
                .enumerate()
            {
                if !audio.continued[side] {
                    mark(&mut edges, at.saturating_sub(1)..at + 2);
                }
            }
        }
        let mut model = Self {
            channels,
            hops,
            lag: 0,
            parts,
            entries,
            gain: vec![[0.0; 2]; plan.audio.len()],
            static_db: &scan.sources.static_db,
            blocked,
            active,
            edges,
            unity: [vec![0.0; hops], vec![0.0; hops]],
            predicted: [vec![0.0; hops], vec![0.0; hops]],
            second: [vec![0.0; hops], vec![0.0; hops]],
            output,
            gain_changes: Vec::new(),
        };
        model.sum(plan, false);
        model.lag = audio_lag_hops(&model, plan, config);
        model.measure_gains(plan, config, normalized);
        model.sum(plan, true);
        model
    }

    fn part_entries(&self, part: usize) -> &'s [SourceHop] {
        self.parts[part]
            .as_ref()
            .map_or(&[][..], |range| &self.entries[range.clone()])
    }

    fn part_power(&self, part: usize, entry: &SourceHop, channel: usize) -> f64 {
        power(entry.level.rms[channel]) * 10_f64.powf(self.static_db[part][channel] / 10.0)
    }

    fn sum(&mut self, plan: &Plan<'_>, measured: bool) {
        // The unity sum stays for the sync pass; only the buffers this pass
        // fills are cleared.
        for channel in 0..2 {
            if measured {
                self.predicted[channel].fill(0.0);
                self.second[channel].fill(0.0);
            } else {
                self.unity[channel].fill(0.0);
            }
        }
        for part in 0..plan.audio.len() {
            let silent = plan.audio[part].silent_channels;
            for entry in self.part_entries(part) {
                let hop = usize::try_from(entry.hop).unwrap_or(usize::MAX);
                if hop >= self.hops {
                    continue;
                }
                for (channel, &quiet) in silent.iter().enumerate().take(self.channels) {
                    if self.channels == 2 && quiet {
                        continue;
                    }
                    let unity = self.part_power(part, entry, channel);
                    #[allow(clippy::cast_possible_truncation, reason = "levels in f32")]
                    if measured {
                        let gain = self.gain[part][channel];
                        self.predicted[channel][hop] += (unity * 10_f64.powf(gain / 10.0)) as f32;
                        self.second[channel][hop] += (linear(entry.level.second[channel])
                            * 10_f64.powf((gain + self.static_db[part][channel]) / 20.0))
                            as f32;
                    } else {
                        self.unity[channel][hop] += unity as f32;
                    }
                }
            }
        }
    }

    /// Output hop `hop` shows predicted hop `hop - lag`.
    fn predicted_hop(&self, hop: usize) -> Option<usize> {
        let at = i64::try_from(hop).ok()? - self.lag;
        usize::try_from(at).ok().filter(|at| *at < self.hops)
    }

    fn output_power(&self, hop: usize, channel: usize) -> f64 {
        self.output
            .hops
            .get(hop)
            .map_or(0.0, |level| power(level.rms[channel]))
    }

    /// Each part's gain over its static mix gain, measured where it
    /// dominates and is loud. A part far quieter than the others (or than
    /// the floor) is treated as dropped, so its hops are judged at the
    /// group's gain.
    ///
    /// R3: the mix fixes every part's level, so an unnormalised export must
    /// measure 0 dB over it and a normalised one a single gain shared by all
    /// parts (their median). A part further than `gain_tolerance_db` from
    /// that is recorded as a gain change, never absorbed.
    fn measure_gains(&mut self, plan: &Plan<'_>, config: &TechnicalScanConfig, normalized: bool) {
        let mut measured: Vec<[Option<f64>; 2]> = vec![[None; 2]; plan.audio.len()];
        for (part, slot) in measured.iter_mut().enumerate() {
            for (channel, value) in slot.iter_mut().enumerate().take(self.channels) {
                let mut differences = Vec::new();
                for entry in self.part_entries(part) {
                    let source = usize::try_from(entry.hop).unwrap_or(usize::MAX);
                    if source >= self.hops || self.blocked[source] {
                        continue;
                    }
                    let own = self.part_power(part, entry, channel);
                    let total = f64::from(self.unity[channel][source]);
                    if own < DOMINANT_SHARE * total || db(own) < f64::from(config.dropout_loud_dbfs)
                    {
                        continue;
                    }
                    let Ok(lagged) = usize::try_from(i64::try_from(source).unwrap_or(0) + self.lag)
                    else {
                        continue;
                    };
                    differences.push(db(self.output_power(lagged, channel)) - db(own));
                }
                *value = median(&mut differences);
            }
        }
        for channel in 0..self.channels {
            let mut all: Vec<f64> = measured.iter().filter_map(|gains| gains[channel]).collect();
            let group = median(&mut all);
            let expected = if normalized {
                group.unwrap_or(0.0)
            } else {
                0.0
            };
            for (part, gains) in measured.iter().enumerate() {
                if let Some(gain) = gains[channel]
                    && (gain - expected).abs() > f64::from(config.gain_tolerance_db)
                {
                    self.gain_changes.push((part, channel, gain - expected));
                }
            }
            let floor = f64::from(config.gain_floor_db);
            let fallback = group.filter(|gain| *gain >= floor).unwrap_or(0.0);
            for (part, gains) in measured.iter().enumerate() {
                let limit = floor.max(group.unwrap_or(0.0) - f64::from(config.gain_drop_db));
                self.gain[part][channel] = match gains[channel] {
                    Some(gain) if gain >= limit => gain,
                    _ => fallback,
                };
            }
        }
    }
}

/// Correlate two equally long envelopes over lags `-radius..=radius`; the
/// best lag and its refined offset when it is a distinct peak above `min`.
fn best_lag(
    pairs: impl Fn(i64) -> Vec<(f64, f64)>,
    radius: i64,
    min_pairs: usize,
    peak_min: f64,
) -> Option<f64> {
    let score = |lag: i64| -> Option<f64> {
        let pairs = pairs(lag);
        if pairs.len() < min_pairs {
            return None;
        }
        #[allow(clippy::cast_precision_loss, reason = "window lengths are small")]
        let count = pairs.len() as f64;
        let (mean_a, mean_b) = pairs
            .iter()
            .fold((0.0, 0.0), |(a, b), (x, y)| (a + x / count, b + y / count));
        let (mut covariance, mut var_a, mut var_b) = (0.0, 0.0, 0.0);
        for (x, y) in &pairs {
            covariance += (x - mean_a) * (y - mean_b);
            var_a += (x - mean_a).powi(2);
            var_b += (y - mean_b).powi(2);
        }
        (var_a > 0.0 && var_b > 0.0).then(|| covariance / (var_a * var_b).sqrt())
    };
    let scores: Vec<(i64, Option<f64>)> = (-radius..=radius).map(|lag| (lag, score(lag))).collect();
    let (best, peak) = scores
        .iter()
        .filter_map(|(lag, value)| value.map(|value| (*lag, value)))
        .max_by(|a, b| a.1.total_cmp(&b.1))?;
    if peak < peak_min {
        return None;
    }
    // A rival is another local maximum, not a neighbour on the slope of
    // the best peak: smooth envelopes have broad peaks.
    let value = |index: usize| scores.get(index).and_then(|(_, value)| *value);
    let rival = (0..scores.len()).any(|index| {
        let Some(candidate) = value(index) else {
            return false;
        };
        let left = index.checked_sub(1).and_then(value);
        let right = value(index + 1);
        (scores[index].0 - best).abs() > 2
            && left.is_none_or(|left| candidate >= left)
            && right.is_none_or(|right| candidate >= right)
            && candidate >= peak - SYNC_DISTINCT
    });
    if rival {
        return None;
    }
    let at = |lag: i64| {
        scores
            .iter()
            .find(|(candidate, _)| *candidate == lag)
            .and_then(|(_, value)| *value)
    };
    #[allow(clippy::cast_precision_loss, reason = "small lags")]
    let refined = match (at(best - 1), at(best + 1)) {
        (Some(left), Some(right)) => {
            let curve = left - 2.0 * peak + right;
            if curve < 0.0 {
                best as f64 + 0.5 * (left - right) / curve
            } else {
                best as f64
            }
        }
        _ => best as f64,
    };
    Some(refined)
}

fn windows(plan: &Plan<'_>, config: &TechnicalScanConfig) -> Vec<Range<u64>> {
    let duration = plan.clock.frame_us(plan.output_frames);
    let window = config.sync_window_us.max(1);
    let count = usize::try_from(duration / window)
        .unwrap_or(usize::MAX)
        .clamp(1, MAX_SYNC_WINDOWS);
    let count_u = u64::try_from(count).unwrap_or(1);
    (0..count_u)
        .map(|index| {
            let start = duration * index / count_u;
            start..(start + window).min(duration)
        })
        .collect()
}

/// Audio lag of one window in hops, from the output and predicted dB
/// envelopes.
fn audio_window_lag(
    model: &Model<'_>,
    window: &Range<u64>,
    config: &TechnicalScanConfig,
) -> Option<f64> {
    let first = usize::try_from(window.start / SCAN_HOP_MICROS).ok()?;
    let last = usize::try_from(window.end / SCAN_HOP_MICROS)
        .ok()?
        .min(model.hops);
    if last <= first {
        return None;
    }
    let envelope = |values: &dyn Fn(usize) -> f64, hop: usize| {
        db((0..model.channels)
            .map(|channel| values(channel * model.hops + hop))
            .sum())
    };
    let predicted = |index: usize| {
        let (channel, hop) = (index / model.hops, index % model.hops);
        f64::from(model.unity[channel][hop])
    };
    let output = |index: usize| {
        let (channel, hop) = (index / model.hops, index % model.hops);
        model.output_power(hop, channel)
    };
    let covered = (first..last)
        .filter(|&hop| model.active[hop] && !model.blocked[hop])
        .count();
    #[allow(clippy::cast_precision_loss, reason = "window lengths are small")]
    if (covered as f64) < SYNC_COVERED_SHARE * (last - first) as f64 {
        return None;
    }
    let source: Vec<f64> = (first..last).map(|hop| envelope(&predicted, hop)).collect();
    let mean = source.iter().sum::<f64>() / f64::from(u32::try_from(source.len()).ok()?);
    let std = (source.iter().map(|v| (v - mean).powi(2)).sum::<f64>()
        / f64::from(u32::try_from(source.len()).ok()?))
    .sqrt();
    if std < SYNC_ENVELOPE_STD_DB {
        return None;
    }
    let radius = i64::try_from(config.sync_search_us / SCAN_HOP_MICROS).ok()?;
    let pairs = |lag: i64| -> Vec<(f64, f64)> {
        (first..last)
            .filter_map(|hop| {
                let shifted = usize::try_from(i64::try_from(hop).ok()? + lag).ok()?;
                (shifted < model.hops).then(|| (envelope(&output, shifted), source[hop - first]))
            })
            .collect()
    };
    let min_pairs = (last - first) / 2;
    best_lag(pairs, radius, min_pairs, f64::from(config.sync_peak_min))
}

fn audio_lag_hops(model: &Model<'_>, plan: &Plan<'_>, config: &TechnicalScanConfig) -> i64 {
    let mut lags: Vec<f64> = windows(plan, config)
        .iter()
        .filter_map(|window| audio_window_lag(model, window, config))
        .collect();
    #[allow(clippy::cast_possible_truncation, reason = "a lag of a few hops")]
    median(&mut lags).map_or(0, |lag| lag.round() as i64)
}

/// Video lag of one window in frames, from the output and source frame
/// difference signatures within compared runs of one part.
fn video_window_lag(
    clock: super::plan::Clock,
    picture: &PictureScan,
    window: &Range<u64>,
    config: &TechnicalScanConfig,
) -> Option<f64> {
    let first = usize::try_from(
        clock.first_frame_from(kinewright_core::ExactMicros::integer(window.start)),
    )
    .ok()?;
    let last =
        usize::try_from(clock.first_frame_from(kinewright_core::ExactMicros::integer(window.end)))
            .ok()?
            .min(picture.frames.len());
    if last <= first + SYNC_MIN_PAIRS {
        return None;
    }
    let frames = &picture.frames;
    // R8: only output frames that show a new source frame carry the
    // source's motion; a repeat (an fps-converted cut's cadence) carries
    // none and would make every cadence period look like a rival lag.
    let source = |index: usize| {
        let frame = frames.get(index)?;
        (frame.state == FrameState::Compared).then_some(())?;
        let repeat = index
            .checked_sub(1)
            .and_then(|previous| frames.get(previous))
            .is_some_and(|previous| {
                previous.part == frame.part && previous.source_frame == frame.source_frame
            });
        (!repeat).then_some(())?;
        frame.src_diff.map(|value| (frame.part, f64::from(value)))
    };
    let motion: Vec<f64> = (first..last)
        .filter_map(|index| source(index).map(|(_, v)| v))
        .collect();
    if motion.len() < SYNC_MIN_PAIRS {
        return None;
    }
    let mean = motion.iter().sum::<f64>() / f64::from(u32::try_from(motion.len()).ok()?);
    let std = (motion.iter().map(|v| (v - mean).powi(2)).sum::<f64>()
        / f64::from(u32::try_from(motion.len()).ok()?))
    .sqrt();
    if std < f64::from(config.sync_motion_std_min) {
        return None;
    }
    let frame_us = clock.frame_us(1).max(1);
    let radius = i64::try_from(config.sync_search_us / frame_us).ok()?.max(1);
    let pairs = |lag: i64| -> Vec<(f64, f64)> {
        (first..last)
            .filter_map(|index| {
                let (part, src) = source(index)?;
                let shifted = usize::try_from(i64::try_from(index).ok()? + lag).ok()?;
                let out = frames.get(shifted)?;
                (out.part == part && out.state == FrameState::Compared).then_some(())?;
                Some((f64::from(out.out_diff?), src))
            })
            .collect()
    };
    best_lag(
        pairs,
        radius,
        SYNC_MIN_PAIRS,
        f64::from(config.sync_peak_min),
    )
}

/// Sync (L10): audio and video lags against their sources over windows
/// spread across the timeline. A signal is measured when at least
/// `min(sync_min_windows, windows)` windows decide it; otherwise the row is
/// Unknown. Only a cut that maps no media picture or no unmuted audio has no
/// sync to measure.
fn sync(
    plan: &Plan<'_>,
    picture: Option<&PictureScan>,
    model: Option<&Model<'_>>,
    config: &TechnicalScanConfig,
) -> (SyncMeasurement, Vec<(FindingKind, FindingVerdict)>) {
    let mut measurement = SyncMeasurement::default();
    let mut findings = Vec::new();
    let has_picture = picture.is_some_and(|scan| {
        scan.frames
            .iter()
            .any(|frame| frame.state == FrameState::Compared)
    });
    let has_audio = model.is_some_and(|model| {
        model
            .active
            .iter()
            .zip(&model.blocked)
            .any(|(active, blocked)| *active && !blocked)
    });
    let windows = windows(plan, config);
    let needed = config
        .sync_min_windows
        .min(u32::try_from(windows.len()).unwrap_or(u32::MAX));
    measurement.windows = u32::try_from(windows.len()).unwrap_or(u32::MAX);
    // Nothing mapped to measure: no sync to judge. Something mapped that
    // no frame or hop can compare (effects, blends, processing, truncation)
    // leaves sync unmeasured, never passed: a media part is mapped whatever
    // its exclusion. Expected audio is likewise any unmuted part, processed
    // or not.
    let mapped_picture = plan.video.iter().any(|part| part.asset.is_some());
    if !mapped_picture || !has_expected_audio(plan) {
        return (measurement, findings);
    }
    let (Some(picture), Some(model)) =
        (picture.filter(|_| has_picture), model.filter(|_| has_audio))
    else {
        for (signal, measured) in [
            (SyncSignal::Audio, has_audio),
            (SyncSignal::Video, has_picture),
        ] {
            if !measured {
                findings.push((
                    FindingKind::SyncUnmeasured {
                        signal,
                        measured: 0,
                        needed: needed.max(1),
                    },
                    Undecided,
                ));
            }
        }
        return (measurement, findings);
    };
    #[allow(clippy::cast_precision_loss, reason = "a lag of a few hops")]
    let mut audio: Vec<f64> = windows
        .iter()
        .filter_map(|window| audio_window_lag(model, window, config))
        .map(|lag| lag * SCAN_HOP_MICROS as f64)
        .collect();
    let frame_us = plan.clock.frame_us(1);
    #[allow(clippy::cast_precision_loss, reason = "a lag of a few frames")]
    let mut video: Vec<f64> = windows
        .iter()
        .filter_map(|window| video_window_lag(plan.clock, picture, window, config))
        .map(|lag| lag * frame_us as f64)
        .collect();
    measurement.audio_windows = u32::try_from(audio.len()).unwrap_or(u32::MAX);
    measurement.video_windows = u32::try_from(video.len()).unwrap_or(u32::MAX);
    #[allow(clippy::cast_possible_truncation, reason = "microsecond lags")]
    {
        measurement.audio_lag_us = median(&mut audio).map(|lag| lag.round() as i64);
        measurement.video_lag_us = median(&mut video).map(|lag| lag.round() as i64);
    }
    for (signal, measured) in [
        (SyncSignal::Audio, measurement.audio_windows),
        (SyncSignal::Video, measurement.video_windows),
    ] {
        if measured < needed {
            findings.push((
                FindingKind::SyncUnmeasured {
                    signal,
                    measured,
                    needed,
                },
                Undecided,
            ));
        }
    }
    if let (Some(audio), Some(video)) = (measurement.audio_lag_us, measurement.video_lag_us) {
        let error = audio - video;
        if error.unsigned_abs() > config.sync_allowed_us {
            findings.push((
                FindingKind::SyncOffset {
                    error_us: error,
                    allowed_us: config.sync_allowed_us,
                },
                Failed,
            ));
        }
    }
    (measurement, findings)
}

#[allow(clippy::too_many_lines, reason = "one rule per sound defect family")]
fn sound_defects(
    row: &mut ReviewRowReport,
    plan: &Plan<'_>,
    sound: Result<&SoundScan, &MediaError>,
    model: Option<&Model<'_>>,
    config: &TechnicalScanConfig,
    end_us: u64,
    early: bool,
) {
    let expected = has_expected_audio(plan);
    let scan = match sound {
        Err(error) => {
            row.push(
                FindingKind::Unreadable {
                    pass: ScanPass::Sound,
                    reason: super::reason(error),
                },
                Undecided,
                [0, end_us],
            );
            return;
        }
        Ok(scan) => scan,
    };
    let Some(output) = &scan.output else {
        // F2: no audio to compare is never a clean sound scan.
        if expected {
            row.push(
                FindingKind::SoundNotCompared {
                    reason: "the encode has no audio stream".into(),
                },
                Undecided,
                [0, end_us],
            );
        }
        return;
    };
    if output.truncated || scan.sources.truncated {
        row.push(
            FindingKind::ScanTruncated {
                pass: ScanPass::Sound,
            },
            Undecided,
            [
                hop_us(u64::try_from(output.hops.len()).unwrap_or(0)),
                end_us,
            ],
        );
        return;
    }
    let Some(model) = model else {
        return;
    };
    for (part, audio) in plan.audio.iter().enumerate() {
        let failed = scan.sources.failed.iter().find(|(index, _)| *index == part);
        let reason = match (audio.muted, audio.uncomparable, failed) {
            (false, Some(reason), _) => reason.to_owned(),
            (false, None, Some((_, reason))) => reason.clone(),
            (true, _, _) | (false, None, None) => continue,
        };
        row.push(
            FindingKind::SoundNotCompared { reason },
            Undecided,
            [
                audio.segment.output.start.floor(),
                audio.segment.output.end.ceil(),
            ],
        );
    }
    for &(part, channel, change) in &model.gain_changes {
        let segment = plan.audio[part].segment;
        #[allow(clippy::cast_possible_truncation, reason = "tenths of a dB")]
        row.push(
            FindingKind::GainChanged {
                channel: u8::try_from(channel).unwrap_or(u8::MAX),
                db_tenths: (change * 10.0).round() as i32,
            },
            Failed,
            [segment.output.start.floor(), segment.output.end.ceil()],
        );
    }
    let context = usize::try_from(config.dropout_context_hops).unwrap_or(0);
    let loud = f64::from(config.dropout_loud_dbfs);
    let drop = f64::from(config.dropout_drop_db);
    // R2: the cut's own extent, not the decoded one; hops the encode does
    // not reach are never checked.
    let expected_hops =
        usize::try_from(u128::from(end_us) / u128::from(SCAN_HOP_MICROS)).unwrap_or(usize::MAX);
    let extent = expected_hops.min(model.hops);
    if early {
        let decoded = hop_us(u64::try_from(model.hops).unwrap_or(0)).min(end_us);
        row.push(
            FindingKind::SoundNotCompared {
                reason: "the encode's audio ends before the cut".into(),
            },
            Undecided,
            [decoded, end_us],
        );
    }
    let blocked_near = |source: usize| {
        (source.saturating_sub(context)..=(source + context).min(model.hops.saturating_sub(1)))
            .any(|hop| model.blocked[hop])
    };
    for channel in 0..model.channels {
        let predicted_db = |source: usize| db(f64::from(model.predicted[channel][source]));
        // Dropout: the output far below the source over loud source context.
        let dropped = |hop: usize| {
            let Some(source) = model.predicted_hop(hop) else {
                return false;
            };
            if blocked_near(source) {
                return false;
            }
            let quietest = (source.saturating_sub(context)
                ..=(source + context).min(model.hops.saturating_sub(1)))
                .map(predicted_db)
                .fold(f64::INFINITY, f64::min);
            quietest >= loud && db(model.output_power(hop, channel)) <= predicted_db(source) - drop
        };
        for range in runs(extent, dropped) {
            if range.end - range.start >= u64::from(config.dropout_min_hops) {
                row.push(
                    FindingKind::Dropout {
                        channel: u8::try_from(channel).unwrap_or(u8::MAX),
                    },
                    Failed,
                    [hop_us(range.start), hop_us(range.end)],
                );
            }
        }
        // Click: an output transient the source does not have, away from
        // the cut's own edges.
        let click = |hop: usize| {
            let Some(source) = model.predicted_hop(hop) else {
                return false;
            };
            if model.edges[source] || blocked_near(source) || !model.active[source] {
                return false;
            }
            let out = output
                .hops
                .get(hop)
                .map_or(0.0, |level| linear(level.second[channel]));
            // The codec smears a source transient over its frame: the source
            // peak counts within `click_context_hops` either side.
            let reach = usize::try_from(config.click_context_hops).unwrap_or(0);
            let predicted = model.second[channel]
                [source.saturating_sub(reach)..(source + reach + 1).min(model.hops)]
                .iter()
                .fold(0.0_f32, |peak, value| peak.max(*value));
            out > f64::from(config.click_ratio) * f64::from(predicted)
                + f64::from(config.click_floor)
        };
        for range in runs(extent, click) {
            row.push(
                FindingKind::Click {
                    channel: u8::try_from(channel).unwrap_or(u8::MAX),
                },
                Failed,
                [hop_us(range.start), hop_us(range.end)],
            );
        }
        // Sound where every kept part is muted or absent.
        let silent = f64::from(config.silent_dbfs);
        let unexpected = |hop: usize| {
            let Some(source) = model.predicted_hop(hop) else {
                return false;
            };
            let near = (source.saturating_sub(context)
                ..=(source + context).min(model.hops.saturating_sub(1)))
                .any(|index| model.active[index]);
            !near && db(model.output_power(hop, channel)) > silent
        };
        if expected {
            for range in runs(extent, unexpected) {
                if range.end - range.start >= u64::from(config.dropout_min_hops) {
                    row.push(
                        FindingKind::UnexpectedAudio,
                        Failed,
                        [hop_us(range.start), hop_us(range.end)],
                    );
                }
            }
        }
    }
    let checked = (0..extent)
        .filter(|&hop| {
            model
                .predicted_hop(hop)
                .is_some_and(|source| !model.blocked[source])
        })
        .count();
    row.cover(
        CoverageUnit::Hops,
        u64::try_from(checked).unwrap_or(0),
        0,
        u64::try_from(expected_hops).unwrap_or(0),
    );
}

/// L11: each kept word's output audio against its source audio over the
/// same span, after the measured gain.
fn retained_speech(
    row: &mut ReviewRowReport,
    inputs: &ReviewScanInputs<'_>,
    plan: &Plan<'_>,
    sound: Result<&SoundScan, &MediaError>,
    model: Option<&Model<'_>>,
    config: &RetainedSpeechConfig,
) {
    let expectations = inputs.expectations;
    if !expectations.kept_words.is_determined() {
        row.push(FindingKind::SpeechNotDetermined, Undecided, [0, 0]);
    }
    for segment in &expectations.uncovered_speech_segments {
        let span = expectations
            .segments
            .iter()
            .find(|candidate| candidate.id == *segment)
            .map_or([0, 0], |candidate| {
                [candidate.output.start.floor(), candidate.output.end.ceil()]
            });
        row.push(FindingKind::SpeechNotCovered, Undecided, span);
    }
    let words: Vec<_> = expectations.kept_words.known().collect();
    let total = u64::try_from(words.len()).unwrap_or(u64::MAX);
    let mut decided = 0_u64;
    let failed_parts: Vec<usize> = sound
        .ok()
        .map(|scan| scan.sources.failed.iter().map(|(part, _)| *part).collect())
        .unwrap_or_default();
    for word in words {
        let span = [word.output.start.floor(), word.output.end.ceil()];
        let index = word.word_index;
        let Some(part) = plan.audio_part(&word.segment) else {
            row.push(FindingKind::WordMuted { word_index: index }, Failed, span);
            decided += 1;
            continue;
        };
        if plan.audio[part].muted {
            row.push(FindingKind::WordMuted { word_index: index }, Failed, span);
            decided += 1;
            continue;
        }
        let Some(model) = model
            .filter(|_| plan.audio[part].uncomparable.is_none() && !failed_parts.contains(&part))
        else {
            row.push(
                FindingKind::WordUnmeasurable { word_index: index },
                Undecided,
                span,
            );
            continue;
        };
        let rate = model.output.rate;
        let start = first_hop(samples_at(word.output.start, rate), rate);
        let end = first_hop(samples_at(word.output.end, rate).saturating_sub(1), rate) + 1;
        let verdict = match word_levels(model, plan, part, start..end, 0) {
            Ok(levels) => match judge_levels(&levels, config) {
                WordCompare::Present => onset(model, plan, part, start, &levels, config),
                other => other.into(),
            },
            Err(other) => other.into(),
        };
        match verdict {
            WordVerdict::Unmeasurable => {
                row.push(
                    FindingKind::WordUnmeasurable { word_index: index },
                    Undecided,
                    span,
                );
            }
            WordVerdict::Ambiguous => {
                row.push(
                    FindingKind::WordAmbiguous { word_index: index },
                    Undecided,
                    span,
                );
            }
            WordVerdict::Missing => {
                decided += 1;
                row.push(
                    FindingKind::WordMissing {
                        word_index: index,
                        text: word.text.clone(),
                    },
                    Failed,
                    span,
                );
            }
            WordVerdict::ClippedStart(onset_span) => {
                decided += 1;
                row.push(
                    FindingKind::WordClippedStart { word_index: index },
                    Failed,
                    [hop_us(onset_span.start), hop_us(onset_span.end)],
                );
            }
            WordVerdict::Present => decided += 1,
        }
    }
    row.cover(CoverageUnit::Words, decided, 0, total);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WordCompare {
    Present,
    Missing,
    Ambiguous,
    Unmeasurable,
}

/// A kept word's verdict. Only `Present`, `Missing` and `ClippedStart` are
/// decided; the row rule is that any undecided word leaves the row Unknown.
#[derive(Debug, Clone, PartialEq, Eq)]
enum WordVerdict {
    Present,
    Missing,
    /// Present, but its first `onset_us` (from the acoustic onset) is gone:
    /// the output hops of that onset.
    ClippedStart(Range<u64>),
    Ambiguous,
    Unmeasurable,
}

impl From<WordCompare> for WordVerdict {
    fn from(compare: WordCompare) -> Self {
        match compare {
            WordCompare::Present => Self::Present,
            WordCompare::Missing => Self::Missing,
            WordCompare::Ambiguous => Self::Ambiguous,
            WordCompare::Unmeasurable => Self::Unmeasurable,
        }
    }
}

/// One hop of a word: the word's own predicted power, all kept audio's
/// predicted power and the output's power, summed over the channels.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
struct HopLevels {
    own: f64,
    all: f64,
    out: f64,
}

/// The word onset starts at the first hop whose own level is within this
/// many dB of the word's loudest hop, so word-box padding before the sound
/// does not count as its onset (R10).
const ONSET_RISE_DB: f64 = 15.0;
/// The onset is compared with the output shifted by up to this many hops
/// either way, so a codec's sub-hop shift does not clip it (R10).
const ONSET_SHIFT_HOPS: i64 = 1;

/// The per-hop levels of output hops `hops` against the word's part and the
/// rest of the kept audio over the same (lagged) source hops, the output
/// read `out_shift` hops later.
fn word_levels(
    model: &Model<'_>,
    plan: &Plan<'_>,
    part: usize,
    hops: Range<u64>,
    out_shift: i64,
) -> Result<Vec<HopLevels>, WordCompare> {
    let silent = plan.audio[part].silent_channels;
    let entries = model.part_entries(part);
    let mut levels = Vec::new();
    for hop in hops {
        let hop = usize::try_from(hop).map_err(|_| WordCompare::Unmeasurable)?;
        let source = model
            .predicted_hop(hop)
            .filter(|source| !model.blocked[*source])
            .ok_or(WordCompare::Unmeasurable)?;
        let shifted = i64::try_from(hop)
            .ok()
            .and_then(|hop| hop.checked_add(out_shift))
            .and_then(|hop| usize::try_from(hop).ok())
            .filter(|hop| *hop < model.output.hops.len())
            .ok_or(WordCompare::Unmeasurable)?;
        let entry = entries
            .binary_search_by_key(&u32::try_from(source).unwrap_or(u32::MAX), |entry| {
                entry.hop
            })
            .ok()
            .map(|at| &entries[at]);
        let mut level = HopLevels::default();
        for (channel, &quiet) in silent.iter().enumerate().take(model.channels) {
            if model.channels == 2 && quiet {
                continue;
            }
            let gain = 10_f64.powf(model.gain[part][channel] / 10.0);
            level.own += entry.map_or(0.0, |entry| model.part_power(part, entry, channel) * gain);
            level.all += f64::from(model.predicted[channel][source]);
            level.out += model.output_power(shifted, channel);
        }
        levels.push(level);
    }
    Ok(levels)
}

/// Judge a word (or its onset) from its per-hop levels.
///
/// Rule: the word's own predicted level must reach `word_floor_dbfs`. Its
/// separation from the rest of the kept audio is weighted by the word's own
/// power per hop (R10), so the hops where the word sounds count, not the
/// pauses inside its box: `OWN = Σ own²`, `ALL = Σ own·all`,
/// `OUT = Σ own·out`, and `D = 10·log10(ALL / (ALL − OWN))` (capped) must
/// reach `separation_min_db`. The word is missing when `OUT` is more than
/// `D / 2` dB below `ALL`, halfway between "word present" (0 dB) and "only
/// the rest present" (−D dB).
fn judge_levels(levels: &[HopLevels], config: &RetainedSpeechConfig) -> WordCompare {
    if levels.is_empty() {
        return WordCompare::Unmeasurable;
    }
    #[allow(clippy::cast_precision_loss, reason = "a word is a few hundred hops")]
    let count = levels.len() as f64;
    let mean_own = levels.iter().map(|level| level.own).sum::<f64>() / count;
    if db(mean_own) < f64::from(config.word_floor_dbfs) {
        return WordCompare::Unmeasurable;
    }
    let (own, all, out) = levels
        .iter()
        .fold((0.0, 0.0, 0.0), |(own, all, out), level| {
            (
                own + level.own * level.own,
                all + level.own * level.all,
                out + level.own * level.out,
            )
        });
    let other = (all - own).max(0.0);
    let separation = if other <= 0.0 {
        f64::from(config.separation_cap_db)
    } else {
        (10.0 * (all / other).log10()).min(f64::from(config.separation_cap_db))
    };
    if separation < f64::from(config.separation_min_db) {
        return WordCompare::Ambiguous;
    }
    if db(out) - db(all) < -separation / 2.0 {
        WordCompare::Missing
    } else {
        WordCompare::Present
    }
}

/// The first `onset_us` of a present word, from its acoustic onset, at
/// output shifts `-ONSET_SHIFT_HOPS..=ONSET_SHIFT_HOPS`: present when any
/// shift finds it, clipped when every shift finds it missing, and undecided
/// otherwise (R3: an onset that cannot be judged leaves the word Unknown).
fn onset(
    model: &Model<'_>,
    plan: &Plan<'_>,
    part: usize,
    start: u64,
    levels: &[HopLevels],
    config: &RetainedSpeechConfig,
) -> WordVerdict {
    let length = config.onset_us.div_ceil(SCAN_HOP_MICROS).max(1);
    let Some(first) = acoustic_onset(levels) else {
        return WordVerdict::Unmeasurable;
    };
    let from = start + u64::try_from(first).unwrap_or(0);
    let to = (from + length).min(start + u64::try_from(levels.len()).unwrap_or(0));
    let verdicts: Vec<WordCompare> = (-ONSET_SHIFT_HOPS..=ONSET_SHIFT_HOPS)
        .map(
            |shift| match word_levels(model, plan, part, from..to, shift) {
                Ok(onset) => judge_levels(&onset, config),
                Err(other) => other,
            },
        )
        .collect();
    onset_verdict(&verdicts, from..to)
}

/// Index of the first hop within `ONSET_RISE_DB` of the loudest own level.
fn acoustic_onset(levels: &[HopLevels]) -> Option<usize> {
    let peak = levels.iter().map(|level| level.own).fold(0.0_f64, f64::max);
    if peak <= 0.0 {
        return None;
    }
    let threshold = peak * 10_f64.powf(-ONSET_RISE_DB / 10.0);
    levels.iter().position(|level| level.own >= threshold)
}

fn onset_verdict(verdicts: &[WordCompare], hops: Range<u64>) -> WordVerdict {
    if verdicts.contains(&WordCompare::Present) {
        WordVerdict::Present
    } else if !verdicts.is_empty() && verdicts.iter().all(|v| *v == WordCompare::Missing) {
        WordVerdict::ClippedStart(hops)
    } else if verdicts.contains(&WordCompare::Ambiguous) {
        WordVerdict::Ambiguous
    } else {
        WordVerdict::Unmeasurable
    }
}

fn joins_row(row: &mut ReviewRowReport, joins: &Result<JoinScan, MediaError>) {
    let scan = match joins {
        Err(error) => {
            row.push(
                FindingKind::Unreadable {
                    pass: ScanPass::Joins,
                    reason: super::reason(error),
                },
                Undecided,
                [0, 0],
            );
            return;
        }
        Ok(scan) => scan,
    };
    let mut checked = 0_u64;
    for join in &scan.joins {
        let at = [join.output_us, join.output_us];
        match &join.verdict {
            JoinVerdict::Clean => checked += 1,
            JoinVerdict::InSound(issue) => {
                checked += 1;
                row.push(
                    FindingKind::JoinInSound {
                        issue: issue.clone(),
                    },
                    Failed,
                    at,
                );
            }
            JoinVerdict::Unmeasured => row.push(FindingKind::JoinUnmeasured, Undecided, at),
        }
    }
    for (asset, reason, _) in &scan.failed {
        row.push(
            FindingKind::ProfileUnavailable {
                asset: *asset,
                reason: reason.clone(),
            },
            Undecided,
            [0, 0],
        );
    }
    if scan.omitted > 0 {
        row.push(
            FindingKind::JoinsOmitted {
                count: scan.omitted,
            },
            Undecided,
            [0, 0],
        );
    }
    row.cover(CoverageUnit::Joins, checked, 0, scan.expected);
}

#[cfg(test)]
#[path = "judge_tests.rs"]
mod tests;
