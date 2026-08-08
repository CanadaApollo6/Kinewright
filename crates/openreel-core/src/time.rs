use std::ops::Range;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// A frame count in the time base named by the surrounding value.
///
/// Project positions use the project's frame rate. `Clip::source_range` and
/// `MediaAsset::duration` use that asset's frame rate.
#[derive(
    Debug, Default, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
    JsonSchema,
)]
#[serde(transparent)]
pub struct TimeCode(pub i64);

impl std::fmt::Display for TimeCode {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(formatter)
    }
}

impl TimeCode {
    pub const ZERO: Self = Self(0);

    #[must_use]
    pub const fn frames(self) -> i64 {
        self.0
    }

    pub fn checked_add(self, rhs: Self) -> Option<Self> {
        self.0.checked_add(rhs.0).map(Self)
    }

    pub fn checked_sub(self, rhs: Self) -> Option<Self> {
        self.0.checked_sub(rhs.0).map(Self)
    }
}

/// A positive rational number, used for exact frame rates such as 24000/1001.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub struct Rational {
    numerator: u32,
    denominator: u32,
}

impl Rational {
    pub fn new(numerator: u32, denominator: u32) -> Result<Self, TimeMappingError> {
        if numerator == 0 || denominator == 0 {
            return Err(TimeMappingError::InvalidRate {
                numerator,
                denominator,
            });
        }

        let divisor = gcd(numerator, denominator);
        Ok(Self {
            numerator: numerator / divisor,
            denominator: denominator / divisor,
        })
    }

    #[must_use]
    pub const fn numerator(self) -> u32 {
        self.numerator
    }

    #[must_use]
    pub const fn denominator(self) -> u32 {
        self.denominator
    }

    #[must_use]
    pub const fn is_valid(self) -> bool {
        self.numerator != 0 && self.denominator != 0
    }
}

impl Default for Rational {
    fn default() -> Self {
        Self {
            numerator: 30,
            denominator: 1,
        }
    }
}

const fn gcd(mut lhs: u32, mut rhs: u32) -> u32 {
    while rhs != 0 {
        let remainder = lhs % rhs;
        lhs = rhs;
        rhs = remainder;
    }
    lhs
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameRounding {
    Floor,
    Nearest,
    Ceil,
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum TimeMappingError {
    #[error("frame rates must have positive numerator and denominator, got {numerator}/{denominator}")]
    InvalidRate { numerator: u32, denominator: u32 },
    #[error("negative frame counts cannot be mapped: {0}")]
    NegativeFrames(TimeCode),
    #[error("frame-rate conversion overflowed")]
    Overflow,
    #[error("source range must be non-empty and non-negative: {start}..{end}")]
    InvalidRange { start: i64, end: i64 },
}

/// Map a frame boundary between time bases using integer, round-to-nearest math.
pub fn map_frames(
    frames: TimeCode,
    source_fps: Rational,
    project_fps: Rational,
) -> Result<TimeCode, TimeMappingError> {
    map_frames_with_rounding(frames, source_fps, project_fps, FrameRounding::Nearest)
}

/// Map a frame boundary between time bases without passing through seconds or floats.
pub fn map_frames_with_rounding(
    frames: TimeCode,
    source_fps: Rational,
    project_fps: Rational,
    rounding: FrameRounding,
) -> Result<TimeCode, TimeMappingError> {
    validate_rate(source_fps)?;
    validate_rate(project_fps)?;
    if frames.0 < 0 {
        return Err(TimeMappingError::NegativeFrames(frames));
    }

    // source frames / source fps * project fps
    let numerator = i128::from(frames.0)
        .checked_mul(i128::from(project_fps.numerator))
        .and_then(|value| value.checked_mul(i128::from(source_fps.denominator)))
        .ok_or(TimeMappingError::Overflow)?;
    let denominator = i128::from(source_fps.numerator)
        .checked_mul(i128::from(project_fps.denominator))
        .ok_or(TimeMappingError::Overflow)?;

    let mapped = match rounding {
        FrameRounding::Floor => numerator / denominator,
        FrameRounding::Nearest => numerator
            .checked_add(denominator / 2)
            .ok_or(TimeMappingError::Overflow)?
            / denominator,
        FrameRounding::Ceil => numerator
            .checked_add(denominator - 1)
            .ok_or(TimeMappingError::Overflow)?
            / denominator,
    };

    i64::try_from(mapped)
        .map(TimeCode)
        .map_err(|_| TimeMappingError::Overflow)
}

/// Map an asset range by mapping both absolute boundaries, then subtracting.
///
/// Mapping absolute boundaries is important: adjacent ranges share the exact
/// same mapped boundary, so mixed-rate splits cannot create cumulative drift.
pub fn map_source_range_to_project(
    source: Range<TimeCode>,
    source_fps: Rational,
    project_fps: Rational,
) -> Result<TimeCode, TimeMappingError> {
    if source.start.0 < 0 || source.end <= source.start {
        return Err(TimeMappingError::InvalidRange {
            start: source.start.0,
            end: source.end.0,
        });
    }
    let start = map_frames(source.start, source_fps, project_fps)?;
    let end = map_frames(source.end, source_fps, project_fps)?;
    end.checked_sub(start).ok_or(TimeMappingError::Overflow)
}

fn validate_rate(rate: Rational) -> Result<(), TimeMappingError> {
    if rate.is_valid() {
        Ok(())
    } else {
        Err(TimeMappingError::InvalidRate {
            numerator: rate.numerator,
            denominator: rate.denominator,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_ntsc_film_boundaries_to_thirty_fps_without_drift() {
        let source = Rational::new(24_000, 1_001).unwrap();
        let project = Rational::new(30, 1).unwrap();

        assert_eq!(map_frames(TimeCode(0), source, project).unwrap(), TimeCode(0));
        assert_eq!(map_frames(TimeCode(24), source, project).unwrap(), TimeCode(30));
        assert_eq!(map_frames(TimeCode(48), source, project).unwrap(), TimeCode(60));
        assert_eq!(
            map_frames(TimeCode(24_000), source, project).unwrap(),
            TimeCode(30_030)
        );
    }

    #[test]
    fn adjacent_ranges_share_the_same_rounded_boundary() {
        let source = Rational::new(24_000, 1_001).unwrap();
        let project = Rational::new(30, 1).unwrap();
        let left = map_source_range_to_project(TimeCode(1)..TimeCode(24), source, project).unwrap();
        let right =
            map_source_range_to_project(TimeCode(24)..TimeCode(48), source, project).unwrap();
        let whole =
            map_source_range_to_project(TimeCode(1)..TimeCode(48), source, project).unwrap();

        assert_eq!(left.checked_add(right), Some(whole));
    }

    #[test]
    fn supports_explicit_rounding_at_half_frames() {
        let source = Rational::new(60, 1).unwrap();
        let project = Rational::new(30, 1).unwrap();

        assert_eq!(
            map_frames_with_rounding(TimeCode(1), source, project, FrameRounding::Floor).unwrap(),
            TimeCode(0)
        );
        assert_eq!(
            map_frames_with_rounding(TimeCode(1), source, project, FrameRounding::Nearest).unwrap(),
            TimeCode(1)
        );
        assert_eq!(
            map_frames_with_rounding(TimeCode(1), source, project, FrameRounding::Ceil).unwrap(),
            TimeCode(1)
        );
    }

    #[test]
    fn rejects_invalid_rates_negative_frames_and_overflow() {
        let project = Rational::new(30, 1).unwrap();
        assert!(Rational::new(0, 1).is_err());
        assert!(map_frames(TimeCode(-1), project, project).is_err());

        let huge = Rational::new(u32::MAX, 1).unwrap();
        assert!(map_frames(TimeCode(i64::MAX), Rational::new(1, 1).unwrap(), huge).is_err());
    }
}
