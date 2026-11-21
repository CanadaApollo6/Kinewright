//! JA-1 S5a commit 5: technical scan and retained-speech checks on the exact
//! encoded bytes of an output review.
//!
//! Each check is a pure function of the verified artifact file, the frozen
//! expectations, the frozen document and its own fixed configuration. It
//! returns typed row reports; the writer host records only their row results.
//! A check that cannot decide reports `Unknown`, never `Pass`.
mod decode;
mod judge;
mod speech;
#[cfg(test)]
mod tests;

use std::fs::File;

use kinewright_core::output_review::{RequiredReviewRow, ReviewExpectations, ReviewRowResult};
use kinewright_core::{AudioLoudness, Document, ExportCancellation, ExportSettings, MediaError};
use serde::{Deserialize, Serialize};

use crate::ReviewOutputTiming;

pub use speech::{
    RecognizedReviewWord, RetainedSpeechConfig, RetainedSpeechReport, ReviewRecognition,
    check_retained_speech, recognize_review_audio, recognize_review_audio_with,
};

/// Findings listed per row; the rest are counted in `omitted`.
pub const MAX_REVIEW_FINDINGS_PER_ROW: usize = 64;
/// Four hours at 60 frames per second. Longer encodes are not decided.
pub const MAX_SCAN_FRAMES: u64 = 864_000;
/// Four hours of 10 ms audio hops.
pub const MAX_SCAN_HOPS: u64 = 1_440_000;
/// Compact audio level hop on the output clock.
pub const SCAN_HOP_MICROS: u64 = 10_000;
/// Luma cells per frame for the flash and black measurements.
pub const SCAN_GRID: (usize, usize) = (16, 9);

/// Fixed thresholds of the technical scan. Their digest is part of the
/// checker configuration that the review job binds before results.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TechnicalScanConfig {
    /// Streaming norm: −14 LUFS integrated.
    pub integrated_lufs_hundredths: i32,
    /// ±1 LU.
    pub tolerance_lu_hundredths: i32,
    /// −1 dBTP.
    pub true_peak_ceiling_dbtp_hundredths: i32,
    /// Allowed audio placement error between consecutive decoded frames.
    pub audio_pts_tolerance_samples: u64,
    /// Allowed audio start and end difference from the picture extent: one
    /// AAC frame of priming or padding.
    pub audio_extent_tolerance_samples: u64,
    /// Landmark event search radius around the expected output instant.
    pub sync_search_us: u64,
    /// Smallest audio onset rise, in centidB over the preceding 50 ms.
    pub sync_onset_rise_cdb: i16,
    /// Smallest frame-to-frame mean luma step that marks a visual event.
    pub sync_visual_step: u8,
    /// Flash: a mean luma step of at least this many 8-bit codes...
    pub flash_step: u8,
    /// ...that returns within this many codes of the level before it...
    pub flash_return: u8,
    /// ...within this many frames.
    pub flash_max_frames: u32,
    /// Black: mean luma at most this (8-bit limited range black is 16)...
    pub black_mean_max: u8,
    /// ...with no cell brighter than this...
    pub black_cell_max: u8,
    /// ...for at least this many frames.
    pub black_min_frames: u32,
    /// Freeze: consecutive frames whose mean absolute luma difference is at
    /// most this many 1/256 codes...
    pub freeze_max_difference: u32,
    /// ...for at least this long.
    pub freeze_min_us: u64,
    /// Click: second-difference peak at least this...
    pub click_min_second_difference: f32,
    /// ...and at least this multiple of its hop's second-difference RMS.
    pub click_crest: f32,
    /// Silent hop: RMS at most this many centidBFS.
    pub silence_rms_cdb: i16,
    /// Dropout: a silent run at most this long...
    pub dropout_max_us: u64,
    /// ...between hops at least this much louder.
    pub dropout_contrast_cdb: i16,
    /// Silence over speech: a silent run at least this long...
    pub silence_gap_min_us: u64,
    /// ...overlapping kept speech by at least this much.
    pub silence_over_speech_min_us: u64,
    /// Audio defects this close to an authored join belong to the join audit.
    pub join_tolerance_us: u64,
}

impl Default for TechnicalScanConfig {
    fn default() -> Self {
        let target = kinewright_core::STREAMING_PLATFORM_TARGET;
        Self {
            integrated_lufs_hundredths: target.integrated_lufs_hundredths,
            tolerance_lu_hundredths: target.tolerance_lu_hundredths,
            true_peak_ceiling_dbtp_hundredths: target.true_peak_ceiling_dbtp_hundredths,
            audio_pts_tolerance_samples: 2,
            audio_extent_tolerance_samples: 1_024,
            sync_search_us: 500_000,
            sync_onset_rise_cdb: 1_500,
            sync_visual_step: 24,
            flash_step: 64,
            flash_return: 16,
            flash_max_frames: 3,
            black_mean_max: 24,
            black_cell_max: 40,
            black_min_frames: 2,
            freeze_max_difference: 64,
            freeze_min_us: 500_000,
            click_min_second_difference: 0.1,
            click_crest: 6.0,
            silence_rms_cdb: -6_000,
            dropout_max_us: 150_000,
            dropout_contrast_cdb: 3_000,
            silence_gap_min_us: 200_000,
            silence_over_speech_min_us: 100_000,
            join_tolerance_us: 20_000,
        }
    }
}

impl TechnicalScanConfig {
    /// Digest of the serialized thresholds.
    ///
    /// # Panics
    /// Never: the configuration is plain numbers.
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
        reason: String,
    },
    TooLong,
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
    AudioGap {
        samples: u64,
    },
    AudioOverlap {
        samples: u64,
    },
    AudioStartLate {
        samples: u64,
    },
    AudioShort {
        samples: u64,
    },
    AudioLong {
        samples: u64,
    },
    LandmarksNotDetermined,
    SyncEventUndetected {
        landmark: u32,
    },
    SyncOffset {
        landmark: u32,
        error_us: u64,
        allowed_us: u64,
    },
    // Loudness and peak.
    LoudnessUnmeasured,
    IntegratedLoudness {
        lufs_hundredths: i32,
    },
    TruePeak {
        dbtp_hundredths: i32,
    },
    // Technical defects.
    Flash,
    Black,
    Freeze,
    Click,
    Dropout,
    SilenceOverSpeech,
    SilenceWithUndeterminedSpeech,
    // Retained speech.
    SpeechNotDetermined,
    SpeechNotCovered,
    RecognitionUnavailable {
        reason: String,
    },
    WordMissing {
        word_index: u32,
        text: String,
    },
    WordAmbiguous {
        word_index: u32,
        text: String,
    },
    WordClippedInCut {
        word_index: u32,
    },
    ProfileUnavailable {
        reason: String,
    },
    JoinInSound {
        issue: String,
    },
    JoinUnmeasured,
    JoinUnderOtherAudio,
    JoinsOmitted {
        count: u64,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReviewFinding {
    pub kind: FindingKind,
    pub verdict: FindingVerdict,
    /// Half-open output microseconds the finding affects.
    pub output_us: [u64; 2],
}

/// One required row: its result and a capped page of what decided it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReviewRowReport {
    pub row: RequiredReviewRow,
    pub result: ReviewRowResult,
    pub findings: Vec<ReviewFinding>,
    pub omitted: u64,
}

impl ReviewRowReport {
    pub(crate) fn new(row: RequiredReviewRow) -> Self {
        Self {
            row,
            result: ReviewRowResult::Pass,
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

    #[must_use]
    pub fn has(&self, matches: impl Fn(&FindingKind) -> bool) -> bool {
        self.findings.iter().any(|finding| matches(&finding.kind))
    }
}

/// The three rows the scan decides.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TechnicalReviewReport {
    pub configuration_sha256: [u8; 32],
    pub structure: ReviewRowReport,
    pub loudness: ReviewRowReport,
    pub defects: ReviewRowReport,
    pub decoded_frames: u64,
    pub decoded_audio_samples: u64,
    pub measured: Option<AudioLoudness>,
}

impl TechnicalReviewReport {
    #[must_use]
    pub fn rows(&self) -> [&ReviewRowReport; 3] {
        [&self.structure, &self.loudness, &self.defects]
    }
}

/// The frozen inputs a scan reads besides the artifact bytes.
pub struct ReviewScanInputs<'a> {
    pub timing: &'a ReviewOutputTiming,
    pub expectations: &'a ReviewExpectations,
    pub document: &'a Document,
    pub settings: &'a ExportSettings,
}

/// Decode the verified artifact in two bounded sequential passes (picture,
/// then sound), and decide the
/// structure, loudness and technical defect rows.
///
/// A decoder failure is not an error: it is an `Unknown` structure row with
/// the remaining rows unrun. Only cancellation returns an error.
///
/// # Errors
/// Returns `MediaError::Cancelled` when the scan is cancelled.
pub fn scan_review_artifact(
    artifact: File,
    inputs: &ReviewScanInputs<'_>,
    config: &TechnicalScanConfig,
    cancellation: &ExportCancellation,
) -> Result<TechnicalReviewReport, MediaError> {
    match decode::scan(artifact, inputs, config, cancellation) {
        Ok(summary) => Ok(judge::judge(&summary, inputs, config)),
        Err(MediaError::Cancelled) => Err(MediaError::Cancelled),
        Err(error) => {
            let mut report = judge::unrun(config);
            report.structure.push(
                FindingKind::Unreadable {
                    reason: error.to_string().chars().take(256).collect(),
                },
                FindingVerdict::Undecided,
                [0, 0],
            );
            Ok(report)
        }
    }
}
