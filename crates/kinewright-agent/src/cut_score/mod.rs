// Derived from Scarlet evals/src/metrics.ts at 2cbab542; see LICENSES/scarlet-MIT.txt.
//! Offline comparison of one recording's dialogue cut with a person's cut.
//! Source coverage and playback occurrences are deliberately separate.

mod document;
mod intervals;
mod metrics;
mod pacing;

pub use document::candidate_from_document;
pub use metrics::{
    Edge, EdgeComparison, EdgeMatch, EdgeSide, EdgeTolerance, MissingLine, RequiredLines, Score,
    score,
};
pub use pacing::{Pacing, PacingGap, PacingRepeat};

use kinewright_core::dialogue::Word;
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// A half-open range of the original recording, in integer microseconds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceRange {
    pub start_micros: u64,
    pub end_micros: u64,
}

/// Inclusive indices in the case's ordered word array.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RequiredLine {
    pub from_word: usize,
    pub to_word: usize,
}

/// Real case files live outside the repository. No audio is read by the scorer.
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Case {
    pub recording_sha256: String,
    pub duration_micros: u64,
    pub request: String,
    pub words: Vec<Word>,
    /// Playback order, including any deliberate repeated source material.
    pub reference_kept: Vec<SourceRange>,
    /// Explicit requirements are unconditional. When absent, infer sentences
    /// required by the reference's keep share.
    pub required_lines: Option<Vec<RequiredLine>>,
}

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Candidate {
    pub recording_sha256: String,
    pub kept: Vec<SourceRange>,
}

/// Exact, adjustable editorial share threshold.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Share {
    pub numerator: u32,
    pub denominator: u32,
}

impl Share {
    fn reached(self, count: usize, total: usize) -> bool {
        (count as u128) * u128::from(self.denominator)
            >= (total as u128) * u128::from(self.numerator)
    }
}

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScoreSettings {
    pub edge_match_window_micros: u64,
    pub edge_tolerances_micros: Vec<u64>,
    pub required_keep_share: Share,
    pub missing_cut_share: Share,
    pub sentence_gap_micros: u64,
    pub gap_micros: u64,
    pub long_gap_micros: u64,
    pub beat_micros: u64,
    pub repeat_within_micros: u64,
    pub repeat_lookahead_micros: u64,
    pub repeat_words: Option<usize>,
}

impl Default for ScoreSettings {
    fn default() -> Self {
        Self {
            edge_match_window_micros: 1_000_000,
            edge_tolerances_micros: vec![20_000, 100_000],
            required_keep_share: Share {
                numerator: 4,
                denominator: 5,
            },
            missing_cut_share: Share {
                numerator: 1,
                denominator: 2,
            },
            sentence_gap_micros: 1_500_000,
            gap_micros: 500_000,
            long_gap_micros: 1_000_000,
            beat_micros: 250_000,
            repeat_within_micros: 3_000_000,
            repeat_lookahead_micros: 25_000_000,
            repeat_words: None,
        }
    }
}

/// Unreduced counts preserve the sample size. Display is rounded half-up to
/// three decimal places by integer arithmetic; zero denominator is undefined.
#[derive(Debug, PartialEq, Eq, Serialize)]
pub struct Ratio {
    pub numerator: u128,
    pub denominator: u128,
    pub display: Option<String>,
}

impl Ratio {
    fn new(numerator: u128, denominator: u128) -> Self {
        // Inputs are counts or sums of u64 source durations. These products
        // fit u128; they never come from unconstrained ratio JSON.
        let display = (numerator * 1_000 + denominator / 2)
            .checked_div(denominator)
            .map(|rounded| format!("{}.{:03}", rounded / 1_000, rounded % 1_000));
        Self {
            numerator,
            denominator,
            display,
        }
    }
}

#[derive(Debug, PartialEq, Eq, Serialize)]
pub struct PrecisionRecall {
    pub true_positive: u128,
    pub false_positive: u128,
    pub false_negative: u128,
    pub precision: Ratio,
    pub recall: Ratio,
    pub f1: Ratio,
}

impl PrecisionRecall {
    fn new(tp: u128, fp: u128, missed: u128) -> Self {
        Self {
            true_positive: tp,
            false_positive: fp,
            false_negative: missed,
            precision: Ratio::new(tp, tp + fp),
            recall: Ratio::new(tp, tp + missed),
            f1: Ratio::new(2 * tp, 2 * tp + fp + missed),
        }
    }
}

#[derive(Debug, PartialEq, Eq, Serialize, Error)]
#[serde(tag = "reason", rename_all = "snake_case")]
pub enum CannotScore {
    #[error("the case or candidate recording hash is invalid")]
    InvalidHash,
    #[error("case and candidate name different recording contents")]
    RecordingMismatch,
    #[error("recording duration must be positive")]
    Duration,
    #[error("word {index} has invalid or unordered source timing or empty text")]
    Word { index: usize },
    #[error("{input} range {index} is empty, reversed or outside the recording")]
    Range { input: &'static str, index: usize },
    #[error("required line {index} is outside the ordered words")]
    RequiredLine { index: usize },
    #[error("score settings have invalid thresholds")]
    Settings,
    #[error("source or playback duration cannot be represented")]
    TimeOverflow,
    #[error("the document is invalid: {detail}")]
    Document { detail: String },
    #[error("the selected asset or dialogue track does not exist")]
    Selection,
    #[error("the selected source has no verified content identity")]
    UnverifiedSource,
    #[error("clip {clip} does not play at speed one")]
    Speed { clip: u64 },
    #[error("the dialogue track has an unsupported shape: {detail}")]
    TrackShape { detail: &'static str },
}

fn valid_hash(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

fn validate_ranges(
    ranges: &[SourceRange],
    duration: u64,
    input: &'static str,
) -> Result<(), CannotScore> {
    let mut played = 0_u64;
    for (index, r) in ranges.iter().enumerate() {
        if r.start_micros >= r.end_micros || r.end_micros > duration {
            return Err(CannotScore::Range { input, index });
        }
        played = played
            .checked_add(r.end_micros - r.start_micros)
            .ok_or(CannotScore::TimeOverflow)?;
    }
    Ok(())
}

fn validate(
    case: &Case,
    candidate: &Candidate,
    settings: &ScoreSettings,
) -> Result<(), CannotScore> {
    if !valid_hash(&case.recording_sha256) || !valid_hash(&candidate.recording_sha256) {
        return Err(CannotScore::InvalidHash);
    }
    if case.recording_sha256 != candidate.recording_sha256 {
        return Err(CannotScore::RecordingMismatch);
    }
    if case.duration_micros == 0 {
        return Err(CannotScore::Duration);
    }
    for (index, w) in case.words.iter().enumerate() {
        if w.text.trim().is_empty()
            || w.start_micros >= w.end_micros
            || w.end_micros > case.duration_micros
            || index > 0
                && (w.start_micros < case.words[index - 1].start_micros
                    || w.end_micros < case.words[index - 1].end_micros)
        {
            return Err(CannotScore::Word { index });
        }
    }
    validate_ranges(&case.reference_kept, case.duration_micros, "reference")?;
    validate_ranges(&candidate.kept, case.duration_micros, "candidate")?;
    if let Some(lines) = &case.required_lines {
        for (index, line) in lines.iter().enumerate() {
            if line.from_word > line.to_word || line.to_word >= case.words.len() {
                return Err(CannotScore::RequiredLine { index });
            }
        }
    }
    if [settings.required_keep_share, settings.missing_cut_share]
        .iter()
        .any(|s| s.denominator == 0 || s.numerator > s.denominator)
        || settings.edge_tolerances_micros.is_empty()
        || settings
            .edge_tolerances_micros
            .iter()
            .any(|&v| v > settings.edge_match_window_micros)
        || settings
            .edge_tolerances_micros
            .windows(2)
            .any(|w| w[0] >= w[1])
        || settings.gap_micros == 0
        || settings.long_gap_micros < settings.gap_micros
        || settings.beat_micros > settings.gap_micros
        || settings.repeat_words == Some(0)
    {
        return Err(CannotScore::Settings);
    }
    Ok(())
}

#[cfg(test)]
mod tests;
