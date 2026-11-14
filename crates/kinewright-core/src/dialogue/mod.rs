// Defaults derived from Scarlet packages/mcp/src/dialogue-join.ts at 2cbab542; see LICENSES/scarlet-MIT.txt.
//! Measured dialogue edges on the recording's integer microsecond clock.
//!
//! This module neither edits a document nor reads audio. A prepared [`Dialogue`]
//! owns one complete measurement, its words and the settings used to derive it.

mod evidence;
mod frame;
mod profile;
mod resolve;

pub use evidence::{Dialogue, Sound};
pub use frame::FramedEdge;
pub use profile::{DEFAULT_HOP_MICROS, Hop, LEVEL_FLOOR, ProfileError, SoundProfile};
pub use resolve::{
    AudibleSide, CutRequest, EdgePlacement, Refusal, RefusalReason, ResolvedCut, ResolvedEdge,
    Shift, pad_edge,
};

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
    pub dip_dbfs_hundredths: i16,
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
            dip_dbfs_hundredths: -5_500,
            in_word_gap_micros: 100_000,
            valley_pause_micros: 100_000,
            quiet_margin_micros: 30_000,
            word_margin_micros: 20_000,
            kept_slack_micros: 50_000,
            removed_slack_micros: 300_000,
            minimum_pause_cut_micros: 40_000,
            // Conservative centidB floors of Scarlet's amplitude thresholds:
            // RMS .01 / peak .05 and RMS .002 / peak .012 respectively.
            quiet_rms_max: -4_000,
            quiet_peak_max: -2_603,
            valley_rms_max: -5_398,
            valley_peak_max: -3_842,
        }
    }
}

#[derive(Debug, PartialEq, Eq, Error)]
pub enum InputError {
    #[error("the measured run is empty, reversed or too short for its interior margin")]
    Span,
    #[error("dialogue settings have an invalid level or overflowing/zero padding")]
    Settings,
    #[error("word {index} has empty text, invalid timing or unordered source times")]
    Word { index: usize },
}

impl Settings {
    fn validate(&self) -> Result<(), InputError> {
        let levels = [
            self.dip_dbfs_hundredths,
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
