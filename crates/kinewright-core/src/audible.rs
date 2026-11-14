//! The unit-speed source clock heard in preview and export. Renderers must
//! conform within one output sample; this definition does not decode audio.
use crate::{Rational, TimeCode};
use serde::Serialize;
use std::cmp::Ordering;
use thiserror::Error;

/// Nonnegative exact microseconds. Reduced, bounded by the u64 time domain.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExactMicros(u128, u64);

impl Serialize for ExactMicros {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeTuple;
        let mut tuple = serializer.serialize_tuple(2)?;
        if let Ok(n) = u64::try_from(self.0) {
            tuple.serialize_element(&n)?;
        } else {
            tuple.serialize_element(&self.0.to_string())?;
        }
        tuple.serialize_element(&self.1)?;
        tuple.end()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
#[error("invalid or unrepresentable audible clock")]
pub struct AudibleClockError;

pub(crate) const fn gcd(mut a: u64, mut b: u64) -> u64 {
    while b != 0 {
        let r = a % b;
        a = b;
        b = r;
    }
    a
}
impl ExactMicros {
    pub const ZERO: Self = Self(0, 1);
    #[must_use]
    pub const fn integer(us: u64) -> Self {
        Self(us as u128, 1)
    }
    /// # Errors
    /// Refuses a zero denominator or a time outside u64 microseconds.
    pub fn new(n: u128, d: u64) -> Result<Self, AudibleClockError> {
        if d == 0 || n.div_ceil(u128::from(d)) > u128::from(u64::MAX) {
            return Err(AudibleClockError);
        }
        let g = gcd(
            u64::try_from(n % u128::from(d)).map_err(|_| AudibleClockError)?,
            d,
        );
        Ok(Self(n / u128::from(g), d / g))
    }
    #[must_use]
    pub const fn numerator(self) -> u128 {
        self.0
    }
    #[must_use]
    pub const fn denominator(self) -> u64 {
        self.1
    }
    #[must_use]
    #[allow(
        clippy::cast_possible_truncation,
        reason = "private representation; constructors bound ceil to u64"
    )]
    pub fn floor(self) -> u64 {
        (self.0 / u128::from(self.1)) as u64
    }

    #[must_use]
    #[allow(
        clippy::cast_possible_truncation,
        reason = "private representation; constructors bound ceil to u64"
    )]
    pub fn ceil(self) -> u64 {
        self.0.div_ceil(u128::from(self.1)) as u64
    }

    /// # Errors
    /// Refuses negative frames, invalid rates, or time overflow.
    pub fn frames(frames: TimeCode, fps: Rational) -> Result<Self, AudibleClockError> {
        if frames.0 < 0 || !fps.is_valid() {
            return Err(AudibleClockError);
        }
        Self::new(
            u128::try_from(frames.0).map_err(|_| AudibleClockError)?
                * 1_000_000
                * u128::from(fps.denominator()),
            u64::from(fps.numerator()),
        )
    }
    /// # Errors
    /// Refuses overflow.
    pub fn checked_add(self, rhs: Self) -> Result<Self, AudibleClockError> {
        let common = gcd(self.1, rhs.1);
        let left_scale = rhs.1 / common;
        let right_scale = self.1 / common;
        let denominator = self.1.checked_mul(left_scale).ok_or(AudibleClockError)?;
        let numerator = self
            .0
            .checked_mul(u128::from(left_scale))
            .and_then(|v| {
                rhs.0
                    .checked_mul(u128::from(right_scale))
                    .and_then(|w| v.checked_add(w))
            })
            .ok_or(AudibleClockError)?;
        Self::new(numerator, denominator)
    }
    /// # Errors
    /// Refuses a negative difference or overflow.
    pub fn checked_sub(self, rhs: Self) -> Result<Self, AudibleClockError> {
        let common = gcd(self.1, rhs.1);
        let left_scale = rhs.1 / common;
        let right_scale = self.1 / common;
        let denominator = self.1.checked_mul(left_scale).ok_or(AudibleClockError)?;
        let numerator = self
            .0
            .checked_mul(u128::from(left_scale))
            .and_then(|v| {
                rhs.0
                    .checked_mul(u128::from(right_scale))
                    .and_then(|w| v.checked_sub(w))
            })
            .ok_or(AudibleClockError)?;
        Self::new(numerator, denominator)
    }
}
impl Ord for ExactMicros {
    fn cmp(&self, rhs: &Self) -> Ordering {
        self.floor().cmp(&rhs.floor()).then_with(|| {
            (self.0 % u128::from(self.1) * u128::from(rhs.1))
                .cmp(&(rhs.0 % u128::from(rhs.1) * u128::from(self.1)))
        })
    }
}
impl PartialOrd for ExactMicros {
    fn partial_cmp(&self, rhs: &Self) -> Option<Ordering> {
        Some(self.cmp(rhs))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct AudibleSpan {
    pub start: ExactMicros,
    pub end: ExactMicros,
    pub padding: ExactMicros,
}
impl AudibleSpan {
    #[must_use]
    pub const fn source(start: u64, end: u64) -> Self {
        Self {
            start: ExactMicros::integer(start),
            end: ExactMicros::integer(end),
            padding: ExactMicros::ZERO,
        }
    }
    /// # Errors
    /// Refuses a reversed source range or overflowing duration.
    pub fn duration(self) -> Result<ExactMicros, AudibleClockError> {
        self.end.checked_sub(self.start)?.checked_add(self.padding)
    }
}

/// Exactly what a speed-one segment plays. Source in-point plus project
/// duration, bounded by the stored source end; any remainder is padding,
/// never evidence of measured quiet.
///
/// # Errors
/// Refuses empty/reversed source ranges, nonpositive project length, invalid
/// rates or arithmetic outside the exact microsecond domain.
pub fn audible_span(
    source: std::ops::Range<TimeCode>,
    asset_rate: Rational,
    project_length: TimeCode,
    project_rate: Rational,
) -> Result<AudibleSpan, AudibleClockError> {
    if source.start >= source.end || project_length.0 <= 0 {
        return Err(AudibleClockError);
    }
    let start = ExactMicros::frames(source.start, asset_rate)?;
    let stored_end = ExactMicros::frames(source.end, asset_rate)?;
    let project_end = start.checked_add(ExactMicros::frames(project_length, project_rate)?)?;
    let end = stored_end.min(project_end);
    Ok(AudibleSpan {
        start,
        end,
        padding: project_end.checked_sub(end)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn rate(n: u32, d: u32) -> Rational {
        Rational::new(n, d).unwrap()
    }
    #[test]
    fn audible_clock_equal_rates_and_padding() {
        assert_eq!(
            audible_span(
                TimeCode(12)..TimeCode(38),
                rate(25, 1),
                TimeCode(26),
                rate(25, 1)
            )
            .unwrap(),
            AudibleSpan::source(480_000, 1_520_000)
        );
        let padded = audible_span(
            TimeCode(0)..TimeCode(24),
            rate(25, 1),
            TimeCode(24),
            rate(24, 1),
        )
        .unwrap();
        assert_eq!(
            padded,
            AudibleSpan {
                start: ExactMicros::ZERO,
                end: ExactMicros::integer(960_000),
                padding: ExactMicros::integer(40_000)
            }
        );
    }
    #[test]
    fn audible_clock_mixed_rate_truncates_the_stored_end() {
        let span = audible_span(
            TimeCode(12)..TimeCode(38),
            rate(25, 1),
            TimeCode(24),
            rate(24, 1),
        )
        .unwrap();
        assert_eq!(
            span,
            AudibleSpan::source(480_000, 1_480_000),
            "DQ_AUDIBLE_END_NOT_STORED_END"
        );
        let ntsc = audible_span(
            TimeCode(1)..TimeCode(31),
            rate(30_000, 1001),
            TimeCode(30),
            rate(30, 1),
        )
        .unwrap();
        assert_eq!(ntsc.start, ExactMicros::new(100_100, 3).unwrap());
        assert_eq!(ntsc.end, ExactMicros::new(3_100_100, 3).unwrap());
        assert_eq!(ntsc.padding, ExactMicros::ZERO);
    }
    #[test]
    fn audible_clock_refuses_invalid_and_compares_exactly() {
        assert!(
            audible_span(
                TimeCode(-1)..TimeCode(2),
                rate(25, 1),
                TimeCode(1),
                rate(25, 1)
            )
            .is_err()
        );
        assert!(ExactMicros::new(1, 0).is_err());
        assert!(ExactMicros::new(u128::MAX, 1).is_err());
        assert!(ExactMicros::new(2, 3).unwrap() < ExactMicros::new(3, 4).unwrap());
        assert_eq!(
            ExactMicros::new(2, 4).unwrap(),
            ExactMicros::new(1, 2).unwrap()
        );
    }
    #[test]
    fn exact_clock_json_keeps_a_large_numerator_exact() {
        let at = ExactMicros::new(u128::from(u64::MAX) * 3 - 1, 3).unwrap();
        assert_eq!(
            serde_json::to_value(at).unwrap(),
            serde_json::json!([(u128::from(u64::MAX) * 3 - 1).to_string(), 3])
        );
    }
    #[test]
    fn audible_grid_is_anchored_to_the_source_in_point() {
        let grid =
            crate::dialogue::FrameGrid::new(ExactMicros::integer(480_000), rate(24, 1)).unwrap();
        let instant = ExactMicros::integer(1_520_000);
        assert_eq!(
            grid.frame_at(instant, crate::FrameRounding::Ceil).unwrap(),
            TimeCode(25)
        );
        assert_eq!(
            grid.frame_at(instant, crate::FrameRounding::Floor).unwrap(),
            TimeCode(24)
        );
        assert_eq!(
            grid.instant(TimeCode(25)).unwrap(),
            ExactMicros::new(4_565_000, 3).unwrap()
        );
    }
}
