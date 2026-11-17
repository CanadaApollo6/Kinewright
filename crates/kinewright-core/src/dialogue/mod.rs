// Defaults derived from Scarlet packages/mcp/src/dialogue-join.ts at 2cbab542; see LICENSES/scarlet-MIT.txt.
//! Measured dialogue edges on the recording's integer microsecond clock.
//!
//! This module neither edits a document nor reads audio. A prepared [`Dialogue`]
//! owns one complete measurement, its words and the settings used to derive it.

pub mod audit;
mod evidence;
mod frame;
pub mod measure;
mod profile;
mod resolve;
mod selection;

pub use evidence::{Dialogue, Sound};
pub use frame::{FrameGrid, FramedEdge};
pub use profile::{
    DEFAULT_HOP_MICROS, Hop, LEVEL_FLOOR, ProfileError, RecordingLevels, SoundProfile,
};
pub use resolve::{
    AudibleSide, CutRequest, DipIntrusion, EdgePlacement, Refusal, RefusalReason, ResolvedCut,
    ResolvedEdge, RetainedAir, Shift, WordBoundary, pad_edge,
};
pub use selection::{SelectionIssue, WORD_ATTRIBUTION_MICROS};

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// A spoken token and its approximate source-clock timing, never a cut edge.
#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Word {
    pub text: String,
    pub start_micros: u64,
    pub end_micros: u64,
}

impl Word {
    fn is_spoken(&self) -> bool {
        let text = self.text.trim();
        !(text.starts_with('[') && text.ends_with(']'))
    }
}

/// A half-open source interval DTO. Consumers validate it against their
/// recording before doing interval arithmetic.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceRange {
    pub start_micros: u64,
    pub end_micros: u64,
}

/// A half-open measured interval, in source microseconds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Span {
    start_micros: u64,
    end_micros: u64,
}

impl Span {
    /// # Errors
    /// Refuses an empty or reversed half-open interval.
    pub fn new(start_micros: u64, end_micros: u64) -> Result<Self, InputError> {
        if start_micros >= end_micros {
            return Err(InputError::Span);
        }
        Ok(Self {
            start_micros,
            end_micros,
        })
    }
    #[must_use]
    pub const fn start_micros(self) -> u64 {
        self.start_micros
    }
    #[must_use]
    pub const fn end_micros(self) -> u64 {
        self.end_micros
    }
    #[must_use]
    pub const fn duration(self) -> u64 {
        self.end_micros - self.start_micros
    }
}

/// All acoustic and timing policy used by one prepared dialogue measurement.
#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Settings {
    pub air_after_micros: u64,
    pub air_before_micros: u64,
    pub snap_limit_micros: u64,
    pub dip_reach_micros: u64,
    pub minimum_word_cut_micros: u64,
    pub in_word_gap_micros: u64,
    pub valley_pause_micros: u64,
    pub quiet_margin_micros: u64,
    pub word_margin_micros: u64,
    pub kept_slack_micros: u64,
    pub removed_slack_micros: u64,
    pub minimum_pause_cut_micros: u64,
    pub quiet_rms_max: i16,
    pub quiet_peak_max: i16,
    pub valley_rms_max: i16,
    pub valley_peak_max: i16,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            air_after_micros: 150_000,
            air_before_micros: 100_000,
            snap_limit_micros: 120_000,
            dip_reach_micros: 50_000,
            minimum_word_cut_micros: 100_000,
            in_word_gap_micros: 100_000,
            valley_pause_micros: 100_000,
            quiet_margin_micros: 30_000,
            word_margin_micros: 20_000,
            kept_slack_micros: 50_000,
            removed_slack_micros: 300_000,
            minimum_pause_cut_micros: 40_000,
            // Conservative centidB floors of Scarlet's amplitude thresholds:
            // RMS .01 / peak .05 and RMS .002 / peak .012 respectively.
            quiet_rms_max: -4_001,
            quiet_peak_max: -2_603,
            valley_rms_max: -5_398,
            valley_peak_max: -3_842,
        }
    }
}

#[derive(Debug, PartialEq, Eq, Error)]
pub enum InputError {
    #[error("recording has no voice-flagged hops to measure its speech level")]
    SpeechLevelUnavailable,
    #[error(
        "speech level {speech_rms_hundredths} and noise floor {noise_floor_hundredths} centidBFS leave no 6 dB-separated quiet/dip threshold (quiet {quiet_rms_hundredths}); use a cleaner recording"
    )]
    NoiseSeparation {
        speech_rms_hundredths: i16,
        noise_floor_hundredths: i16,
        quiet_rms_hundredths: i16,
    },
    #[error("the measured run is empty, reversed or too short for its interior margin")]
    Span,
    #[error("dialogue settings have an invalid level or overflowing/zero padding")]
    Settings,
    #[error("word {index} has empty text, invalid timing or unordered source times")]
    Word { index: usize },
}

/// Speech level for the normalized dialogue on which the reference thresholds were tuned.
pub const REFERENCE_SPEECH: i16 = -2_000;
/// Effective centidBFS limits. Unlike measured levels, a shifted threshold may
/// lie below the measurement floor; saturating it would create false dips.
#[derive(Debug, PartialEq, Eq, Serialize)]
pub struct Thresholds {
    pub quiet_rms_max: i16,
    pub quiet_peak_max: i16,
    pub valley_rms_max: i16,
    pub valley_peak_max: i16,
    /// Measured floor/speech midpoint, capped at quiet RMS and at least 6 dB
    /// above the floor. Without a measured floor no dip is evidence.
    pub dip_dbfs_hundredths: Option<i16>,
}
impl Thresholds {
    fn relative(settings: &Settings, profile: &SoundProfile) -> Result<Self, InputError> {
        let levels = profile.levels();
        let speech = levels
            .speech_rms_hundredths
            .ok_or(InputError::SpeechLevelUnavailable)?;
        let shift = i32::from(speech) - i32::from(REFERENCE_SPEECH);
        let shifted = |db: i16| {
            i16::try_from(i32::from(db) + shift)
                .expect("validated reference limit and speech shift fit i16")
        };
        let mut thresholds = Self {
            quiet_rms_max: shifted(settings.quiet_rms_max),
            quiet_peak_max: shifted(settings.quiet_peak_max),
            valley_rms_max: shifted(settings.valley_rms_max),
            valley_peak_max: shifted(settings.valley_peak_max),
            dip_dbfs_hundredths: None,
        };
        if let Some(floor) = levels.noise_floor_hundredths {
            let lower = i32::from(floor) + 600;
            let upper = i32::from(thresholds.quiet_rms_max).min(i32::from(speech) - 1);
            if lower > upper {
                return Err(InputError::NoiseSeparation {
                    speech_rms_hundredths: speech,
                    noise_floor_hundredths: floor,
                    quiet_rms_hundredths: thresholds.quiet_rms_max,
                });
            }
            let midpoint = (i32::from(floor) + i32::from(speech)).div_euclid(2);
            thresholds.dip_dbfs_hundredths = Some(
                i16::try_from(midpoint.clamp(lower, upper))
                    .expect("measured floor and speech bounds fit i16"),
            );
        }
        Ok(thresholds)
    }
}
impl Settings {
    fn validate(&self) -> Result<(), InputError> {
        let levels = [
            self.quiet_rms_max,
            self.quiet_peak_max,
            self.valley_rms_max,
            self.valley_peak_max,
        ];
        if levels.into_iter().any(|v| !(LEVEL_FLOOR..=0).contains(&v))
            || self
                .air_after_micros
                .checked_add(self.air_before_micros)
                .is_none_or(|v| v == 0)
            || self.quiet_margin_micros == 0
            || self.quiet_margin_micros.checked_mul(2).is_none()
            || self.word_margin_micros.checked_mul(2).is_none()
        {
            return Err(InputError::Settings);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod rehearsal_tests;

#[cfg(test)]
mod r4_tests;

#[cfg(test)]
mod r5_tests;

#[cfg(test)]
mod r6_tests;
