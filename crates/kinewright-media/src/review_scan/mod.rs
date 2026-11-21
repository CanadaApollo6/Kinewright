//! JA-1 S5a commit 5: technical and retained-speech checks of an output
//! review, judged against the sources the cut maps (lead rulings L10 to L13).
//!
//! Each check is a pure function of the verified artifact bytes, the frozen
//! expectations, the frozen document, the pinned owned inputs it maps and its
//! own fixed configuration. Nothing reads the working edit.
//!
//! - Edit-level defects (flash frames, timeline holes, silence over speech)
//!   are decided exactly from the expectation record and the document.
//! - Encode-level defects (freeze, black, picture mismatch, dropout, click)
//!   compare the decoded encode with the pinned source at mapped times.
//! - Sync correlates the output against its sources over windows spread
//!   across the timeline.
//! - Loudness is judged against the target the frozen export settings ask
//!   for.
//! - Retained speech compares each kept word's output audio with its source
//!   audio, and guards every speech join on the source PCM with the source
//!   transcript's word spans. No recognition runs.
//!
//! A check that cannot decide a span reports `Unknown`, never `Pass`. A pass
//! that fails leaves only the rows that depend on it `Unknown`.
mod grid;
mod judge;
mod picture;
mod plan;
mod reader;
mod sound;
mod speech;
#[cfg(test)]
mod tests;

use std::fs::File;

use kinewright_core::output_review::{RequiredReviewRow, ReviewExpectations, ReviewRowResult};
use kinewright_core::{
    AssetId, AudioLoudness, Document, ExportCancellation, ExportSettings, MediaError,
};
use serde::{Deserialize, Serialize};

use crate::ReviewOutputTiming;
use crate::owned_io::CertifiedSource;

/// Findings listed per row; the rest are counted in `omitted`.
pub const MAX_REVIEW_FINDINGS_PER_ROW: usize = 64;
/// Four hours at 60 frames per second. Longer encodes are not decided.
pub const MAX_SCAN_FRAMES: u64 = 864_000;
/// Four hours of 10 ms audio hops.
pub const MAX_SCAN_HOPS: u64 = 1_440_000;
/// Compact audio level hop on the output clock.
pub const SCAN_HOP_MICROS: u64 = 10_000;
/// Irregular PTS, gaps and overlaps listed before only counting.
pub const MAX_TIMING_EVENTS: usize = 64;
/// Excluded spans listed before only counting.
pub const MAX_EXCLUDED_SPANS: usize = 256;
/// Sync windows spread over the timeline.
pub const MAX_SYNC_WINDOWS: usize = 24;
/// Speech joins guarded per review; more leaves the speech row undecided.
pub const MAX_JOIN_EDGES: usize = 32_768;
/// Per-segment source hop entries: every kept track's audio, four deep.
pub const MAX_SOURCE_HOP_ENTRIES: u64 = 4 * MAX_SCAN_HOPS;
/// Source audio decoded around joins, in 10 ms hops.
pub const MAX_JOIN_PROFILE_HOPS: u64 = MAX_SCAN_HOPS;
/// Source video decoders held open at once.
pub const MAX_OPEN_SOURCES: usize = 4;
/// Luma cells per frame for every picture comparison.
pub const LUMA_GRID: (usize, usize) = (32, 18);

/// The checks' fixed decision rules. Their digest is part of the checker
/// configuration the caller binds before results.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TechnicalScanConfig {
    /// Frames decoded before the scan stops and every picture row is Unknown.
    pub max_frames: u64,
    /// Output hops decoded before the scan stops and every sound row is Unknown.
    pub max_hops: u64,
    /// Allowed audio placement error between consecutive decoded frames.
    pub audio_pts_tolerance_samples: u64,
    /// Luma cell mean absolute change (0..1) at which a mapped source frame
    /// counts as moving.
    pub motion_floor: f32,
    /// Freeze: the output changes by at most this fraction of the source...
    pub freeze_ratio: f32,
    /// ...on at least this many moving source frames of one run.
    pub freeze_min_frames: u32,
    /// Black: output mean luma at most this...
    pub black_mean_max: f32,
    /// ...and no cell brighter than this...
    pub black_cell_max: f32,
    /// ...while the mapped source mean is above this or a source cell above
    /// the next.
    pub black_source_mean_min: f32,
    pub black_source_cell_min: f32,
    /// Mismatch: correlation of textured output and source cells below this...
    pub mismatch_correlation_min: f32,
    /// ...or a mean absolute cell difference above this.
    pub mismatch_difference_max: f32,
    /// Cells with less standard deviation than this are flat: compared by
    /// level, not correlation.
    pub texture_min: f32,
    /// Mismatch runs at most this long are flash frames.
    pub flash_max_frames: u32,
    /// Dropout: the predicted level is at least this many dBFS around the
    /// hop...
    pub dropout_loud_dbfs: f32,
    /// ...over this many hops on each side...
    pub dropout_context_hops: u32,
    /// ...and the output is at least this many dB below it...
    pub dropout_drop_db: f32,
    /// ...for at least this many consecutive hops.
    pub dropout_min_hops: u32,
    /// A kept segment whose measured gain is this far below the median of the
    /// others, or below `gain_floor_db`, is treated as dropped.
    pub gain_drop_db: f32,
    pub gain_floor_db: f32,
    /// Click: an output second-difference peak above this multiple of the
    /// predicted peak plus the floor.
    pub click_ratio: f32,
    pub click_floor: f32,
    /// Hops either side whose source peak a codec may smear into the hop.
    pub click_context_hops: u32,
    /// Sync: a window whose source frame-difference signature varies by less
    /// than this is skipped. Lower than `motion_floor`: correlation is
    /// scale-free, so a dim room's small but real motion still measures.
    pub sync_motion_std_min: f32,
    /// Output hops quieter than this are silent (an unexpected stream).
    pub silent_dbfs: f32,
    /// Sync: window length, lag search radius, smallest normalised peak, the
    /// least windows per signal and the largest allowed A/V error.
    pub sync_window_us: u64,
    pub sync_search_us: u64,
    pub sync_peak_min: f32,
    pub sync_min_windows: u32,
    pub sync_allowed_us: u64,
    /// True-peak ceiling when the export does not normalise loudness.
    pub unnormalized_true_peak_ceiling_dbtp_hundredths: i32,
}

impl Default for TechnicalScanConfig {
    fn default() -> Self {
        Self {
            max_frames: MAX_SCAN_FRAMES,
            max_hops: MAX_SCAN_HOPS,
            audio_pts_tolerance_samples: 2,
            motion_floor: 0.004,
            freeze_ratio: 0.25,
            freeze_min_frames: 3,
            black_mean_max: 0.04,
            black_cell_max: 0.08,
            black_source_mean_min: 0.08,
            black_source_cell_min: 0.16,
            mismatch_correlation_min: 0.5,
            mismatch_difference_max: 0.2,
            texture_min: 0.01,
            flash_max_frames: 2,
            dropout_loud_dbfs: -50.0,
            dropout_context_hops: 3,
            dropout_drop_db: 12.0,
            dropout_min_hops: 2,
            gain_drop_db: 15.0,
            gain_floor_db: -40.0,
            click_ratio: 4.0,
            click_floor: 0.05,
            click_context_hops: 2,
            sync_motion_std_min: 0.000_5,
            silent_dbfs: -60.0,
            sync_window_us: 2_000_000,
            sync_search_us: 200_000,
            sync_peak_min: 0.6,
            sync_min_windows: 3,
            sync_allowed_us: 45_000,
            unnormalized_true_peak_ceiling_dbtp_hundredths: 0,
        }
    }
}

impl TechnicalScanConfig {
    /// Digest of the serialized rules.
    #[must_use]
    pub fn sha256(&self) -> [u8; 32] {
        config_digest(self)
    }
}

/// Retained-speech decision rules.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RetainedSpeechConfig {
    /// A kept word whose own predicted level is below this cannot be heard
    /// apart from encoder noise: Unknown.
    pub word_floor_dbfs: f32,
    /// A word whose separation from the other kept audio under it is below
    /// this many dB is ambiguous: Unknown.
    pub separation_min_db: f32,
    /// Separation above this is treated as this.
    pub separation_cap_db: f32,
    /// The onset region compared for a clipped start.
    pub onset_us: u64,
    /// Source audio decoded on each side of a speech join.
    pub join_window_us: u64,
    /// Sample rate of the join profile.
    pub join_profile_rate: u32,
}

impl Default for RetainedSpeechConfig {
    fn default() -> Self {
        Self {
            word_floor_dbfs: -65.0,
            separation_min_db: 6.0,
            separation_cap_db: 24.0,
            onset_us: 50_000,
            join_window_us: 2_000_000,
            join_profile_rate: 16_000,
        }
    }
}

impl RetainedSpeechConfig {
    #[must_use]
    pub fn sha256(&self) -> [u8; 32] {
        config_digest(self)
    }
}

pub(crate) fn config_digest(value: &impl Serialize) -> [u8; 32] {
    let bytes = serde_json::to_vec(value).expect("plain configuration serializes");
    let mut digest = kinewright_hash::Sha256::new();
    digest.update(&bytes);
    digest.finalize()
}

/// Whether a finding fails the row or leaves it undecided. Both block a seal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum FindingVerdict {
    Failed,
    Undecided,
}

/// One bounded decode pass. A pass that fails leaves its rows Unknown.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScanPass {
    Picture,
    Sound,
    Joins,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum FindingKind {
    // Structure and A/V integrity.
    DocumentQa {
        code: String,
    },
    DeliverySettings {
        code: String,
    },
    DeliveryTag {
        field: String,
    },
    Unreadable {
        pass: ScanPass,
        reason: String,
    },
    ScanTruncated {
        pass: ScanPass,
    },
    TimingRecordChanged,
    VideoFrameGrid,
    VideoPts {
        frame: u64,
    },
    FrameCount {
        expected: u64,
        decoded: u64,
    },
    AudioStreamMissing,
    ExtraAudioStream {
        streams: u32,
    },
    UnexpectedAudio,
    AudioPtsMissing,
    AudioGap {
        samples: u64,
    },
    AudioOverlap {
        samples: u64,
    },
    AudioStartLate {
        samples: u64,
    },
    AudioEarly {
        samples: u64,
    },
    AudioShort {
        samples: u64,
    },
    AudioLong {
        samples: u64,
    },
    SyncUnmeasured {
        signal: SyncSignal,
        measured: u32,
        needed: u32,
    },
    SyncOffset {
        error_us: i64,
        allowed_us: u64,
    },
    // Loudness and peak.
    LoudnessUnmeasured,
    IntegratedLoudness {
        lufs_hundredths: i32,
        target_hundredths: i32,
    },
    TruePeak {
        dbtp_hundredths: i32,
        ceiling_hundredths: i32,
    },
    LoudnessRange {
        lu_hundredths: i32,
        max_hundredths: i32,
    },
    // Edit-level defects, from the document.
    EditFlash {
        frames: u64,
    },
    TimelineHole,
    /// A clip's span on the timeline could not be determined: holes are not
    /// decided.
    TimelineUndetermined,
    SilenceOverSpeech {
        word_index: u32,
    },
    // Encode-level defects, output against source.
    Freeze,
    Black,
    Flash,
    PictureMismatch,
    PictureNotCompared {
        reason: String,
    },
    Dropout {
        channel: u8,
    },
    Click {
        channel: u8,
    },
    SoundNotCompared {
        reason: String,
    },
    // Retained speech.
    SpeechNotDetermined,
    SpeechNotCovered,
    WordMuted {
        word_index: u32,
    },
    WordMissing {
        word_index: u32,
        text: String,
    },
    WordClippedStart {
        word_index: u32,
    },
    WordAmbiguous {
        word_index: u32,
    },
    WordUnmeasurable {
        word_index: u32,
    },
    ProfileUnavailable {
        asset: AssetId,
        reason: String,
    },
    JoinInSound {
        issue: String,
    },
    JoinUnmeasured,
    JoinsOmitted {
        count: u64,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SyncSignal {
    Audio,
    Video,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReviewFinding {
    pub kind: FindingKind,
    pub verdict: FindingVerdict,
    /// Half-open output microseconds the finding affects.
    pub output_us: [u64; 2],
}

/// What a row compared, in its own unit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CoverageUnit {
    Frames,
    Hops,
    Samples,
    Words,
    Joins,
    Windows,
}

/// `checked` of `expected` units were compared. Spans another check owns
/// count as `delegated`, never as checked.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RowCoverage {
    pub unit: CoverageUnit,
    pub checked: u64,
    pub delegated: u64,
    pub expected: u64,
}

/// One required row: its result, its coverage and a capped page of what
/// decided it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReviewRowReport {
    pub row: RequiredReviewRow,
    pub result: ReviewRowResult,
    pub coverage: Vec<RowCoverage>,
    pub findings: Vec<ReviewFinding>,
    pub omitted: u64,
}

impl ReviewRowReport {
    pub(crate) fn new(row: RequiredReviewRow) -> Self {
        Self {
            row,
            result: ReviewRowResult::Pass,
            coverage: Vec::new(),
            findings: Vec::new(),
            omitted: 0,
        }
    }

    pub(crate) fn push(&mut self, kind: FindingKind, verdict: FindingVerdict, output_us: [u64; 2]) {
        self.result = match (self.result, verdict) {
            (ReviewRowResult::Failed, _) | (_, FindingVerdict::Failed) => ReviewRowResult::Failed,
            _ => ReviewRowResult::Unknown,
        };
        if self.findings.len() < MAX_REVIEW_FINDINGS_PER_ROW {
            self.findings.push(ReviewFinding {
                kind,
                verdict,
                output_us,
            });
        } else {
            self.omitted = self.omitted.saturating_add(1);
        }
    }

    pub(crate) fn cover(
        &mut self,
        unit: CoverageUnit,
        checked: u64,
        delegated: u64,
        expected: u64,
    ) {
        self.coverage.push(RowCoverage {
            unit,
            checked,
            delegated,
            expected,
        });
    }

    #[must_use]
    pub fn has(&self, matches: impl Fn(&FindingKind) -> bool) -> bool {
        self.findings.iter().any(|finding| matches(&finding.kind))
    }
}

/// Why a span was not compared by the picture checks. Each is owned by the
/// overlay and visual checks of commit 6.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExclusionReason {
    /// More than one video segment covers the frame (picture-in-picture,
    /// stacked layers, a transition's overlap).
    Composited,
    /// The clip has effects or a non-normal blend mode.
    Effect,
    /// Inside a clip's incoming transition.
    Transition,
    /// An overlay whose pixel bounds are not resolved covers the frame.
    OverlayUnresolved,
    /// Overlays cover every comparison cell.
    OverlayCovered,
    /// A non-media clip (title, card, solid, generated).
    Generated,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExcludedSpan {
    pub reason: ExclusionReason,
    pub output_us: [u64; 2],
}

/// What sync measured.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SyncMeasurement {
    /// Windows that held both mapped audio and mapped picture.
    pub windows: u32,
    pub audio_windows: u32,
    pub video_windows: u32,
    /// Median output-minus-source lags.
    pub audio_lag_us: Option<i64>,
    pub video_lag_us: Option<i64>,
}

/// The four rows commit 5 decides.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReviewCheckReport {
    pub technical_sha256: [u8; 32],
    pub speech_sha256: [u8; 32],
    pub structure: ReviewRowReport,
    pub loudness: ReviewRowReport,
    pub defects: ReviewRowReport,
    pub speech: ReviewRowReport,
    /// Picture spans the overlay check owns, merged per reason.
    pub excluded: Vec<ExcludedSpan>,
    pub excluded_omitted: u64,
    pub decoded_frames: u64,
    pub decoded_audio_samples: u64,
    pub measured: Option<AudioLoudness>,
    pub sync: SyncMeasurement,
}

impl ReviewCheckReport {
    #[must_use]
    pub fn rows(&self) -> [&ReviewRowReport; 4] {
        [&self.structure, &self.loudness, &self.defects, &self.speech]
    }
}

/// Pinned owned inputs a check may decode, by asset.
pub trait ReviewSources {
    /// # Errors
    /// The asset has no pinned input.
    fn source(&self, asset: AssetId) -> Result<CertifiedSource, MediaError>;
}

impl ReviewSources for crate::owned_io::PlaybackInputs {
    fn source(&self, asset: AssetId) -> Result<CertifiedSource, MediaError> {
        Self::source(self, asset)
    }
}

/// The frozen inputs a check reads besides the artifact bytes.
pub struct ReviewScanInputs<'a> {
    pub timing: &'a ReviewOutputTiming,
    pub expectations: &'a ReviewExpectations,
    pub document: &'a Document,
    pub settings: &'a ExportSettings,
    pub sources: &'a dyn ReviewSources,
}

/// Decode the verified artifact and its mapped sources in bounded passes and
/// decide the structure, loudness, technical-defect and retained-speech rows.
///
/// The picture, sound and join passes are independent: a pass that cannot
/// read its media leaves only the rows that depend on it Unknown. The
/// artifact is read by one pass at a time from offset 0.
///
/// # Errors
/// Returns `MediaError::Cancelled` when cancelled. Nothing else is an error.
pub fn check_review_artifact(
    artifact: &File,
    inputs: &ReviewScanInputs<'_>,
    technical: &TechnicalScanConfig,
    speech: &RetainedSpeechConfig,
    cancellation: &ExportCancellation,
) -> Result<ReviewCheckReport, MediaError> {
    crate::initialize_ffmpeg()?;
    let plan = plan::Plan::new(inputs);
    let picture = keep_cancel(picture::scan(
        artifact,
        inputs,
        &plan,
        technical,
        cancellation,
    ))?;
    let sound = keep_cancel(sound::scan(
        artifact,
        inputs,
        &plan,
        technical,
        cancellation,
    ))?;
    let joins = keep_cancel(speech::guard_joins(inputs, &plan, speech, cancellation))?;
    Ok(judge::judge(
        inputs, &plan, &picture, &sound, &joins, technical, speech,
    ))
}

/// Cancellation stops the whole check; any other failure stays in the pass.
fn keep_cancel<T>(result: Result<T, MediaError>) -> Result<Result<T, MediaError>, MediaError> {
    match result {
        Err(MediaError::Cancelled) => Err(MediaError::Cancelled),
        other => Ok(other),
    }
}

pub(crate) fn reason(error: &impl std::fmt::Display) -> String {
    error.to_string().chars().take(256).collect()
}
