use std::cmp::Ordering;

use kinewright_core::{Rational, TimeCode};

use crate::{Refusal, RefusalCode, check_rate};

/// Exact, reduced rational seconds or a dimensionless ratio.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Time {
    numerator: i64,
    denominator: u64,
}

impl Default for Time {
    fn default() -> Self {
        Self::ZERO
    }
}

impl Ord for Time {
    fn cmp(&self, other: &Self) -> Ordering {
        (i128::from(self.numerator) * i128::from(other.denominator))
            .cmp(&(i128::from(other.numerator) * i128::from(self.denominator)))
    }
}

impl PartialOrd for Time {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

fn gcd(mut a: u128, mut b: u128) -> u128 {
    while b != 0 {
        (a, b) = (b, a % b);
    }
    a
}

impl Time {
    pub const ZERO: Self = Self::integer(0);
    pub const ONE: Self = Self::integer(1);

    #[must_use]
    pub const fn integer(numerator: i64) -> Self {
        Self {
            numerator,
            denominator: 1,
        }
    }

    /// # Errors
    /// Refuses a zero denominator.
    pub fn new(numerator: i64, denominator: u64) -> Result<Self, Refusal> {
        Self::wide(i128::from(numerator), u128::from(denominator))
    }

    fn wide(numerator: i128, denominator: u128) -> Result<Self, Refusal> {
        if denominator == 0 {
            return Err(Refusal::arithmetic());
        }
        let divisor = gcd(numerator.unsigned_abs(), denominator);
        let magnitude = numerator.unsigned_abs() / divisor;
        let signed = if numerator < 0 {
            -i128::try_from(magnitude).map_err(|_| Refusal::arithmetic())?
        } else {
            i128::try_from(magnitude).map_err(|_| Refusal::arithmetic())?
        };
        Ok(Self {
            numerator: i64::try_from(signed).map_err(|_| Refusal::arithmetic())?,
            denominator: u64::try_from(denominator / divisor).map_err(|_| Refusal::arithmetic())?,
        })
    }

    #[must_use]
    pub const fn numerator(self) -> i64 {
        self.numerator
    }

    #[must_use]
    pub const fn denominator(self) -> u64 {
        self.denominator
    }

    /// # Errors
    /// Refuses a sum outside the representable range.
    pub fn checked_add(self, other: Self) -> Result<Self, Refusal> {
        self.sum(other, i128::checked_add)
    }

    /// # Errors
    /// Refuses a difference outside the representable range.
    pub fn checked_sub(self, other: Self) -> Result<Self, Refusal> {
        self.sum(other, i128::checked_sub)
    }

    fn sum(self, other: Self, combine: fn(i128, i128) -> Option<i128>) -> Result<Self, Refusal> {
        let divisor = u64::try_from(gcd(self.denominator.into(), other.denominator.into()))
            .map_err(|_| Refusal::arithmetic())?;
        let lhs = i128::from(self.numerator) * i128::from(other.denominator / divisor);
        let rhs = i128::from(other.numerator) * i128::from(self.denominator / divisor);
        Self::wide(
            combine(lhs, rhs).ok_or_else(Refusal::arithmetic)?,
            u128::from(self.denominator / divisor) * u128::from(other.denominator),
        )
    }

    /// # Errors
    /// Refuses a product outside the representable range.
    pub fn checked_mul(self, other: Self) -> Result<Self, Refusal> {
        Self::wide(
            i128::from(self.numerator) * i128::from(other.numerator),
            u128::from(self.denominator) * u128::from(other.denominator),
        )
    }

    /// # Errors
    /// Refuses division by zero or an unrepresentable quotient.
    pub fn checked_div(self, other: Self) -> Result<Self, Refusal> {
        let numerator = i128::from(self.numerator) * i128::from(other.denominator);
        Self::wide(
            if other.numerator < 0 {
                -numerator
            } else {
                numerator
            },
            u128::from(self.denominator) * u128::from(other.numerator.unsigned_abs()),
        )
    }

    /// # Errors
    /// Refuses a magnitude outside the representable range.
    pub fn abs(self) -> Result<Self, Refusal> {
        Ok(Self {
            numerator: self
                .numerator
                .checked_abs()
                .ok_or_else(Refusal::arithmetic)?,
            denominator: self.denominator,
        })
    }

    #[must_use]
    pub fn rate(rate: Rational) -> Self {
        Self {
            numerator: i64::from(rate.numerator()),
            denominator: u64::from(rate.denominator()),
        }
    }

    /// # Errors
    /// Refuses a rate outside the core terms or the interchange rate cap.
    pub fn as_rate(self) -> Result<Rational, Refusal> {
        let invalid = || {
            Refusal::new(
                RefusalCode::UnsupportedRate,
                "The frame rate is unsupported.",
            )
        };
        let numerator = u32::try_from(self.numerator).map_err(|_| invalid())?;
        let denominator = u32::try_from(self.denominator).map_err(|_| invalid())?;
        check_rate(Rational::new(numerator, denominator).map_err(|_| invalid())?)
    }

    /// # Errors
    /// Refuses a frame boundary outside the representable range.
    pub fn frame(self, fps: Rational) -> Result<TimeCode, Refusal> {
        let (n, d) = self.frame_terms(fps)?;
        i64::try_from((n + d / 2).div_euclid(d))
            .map(TimeCode)
            .map_err(|_| Refusal::arithmetic())
    }

    /// # Errors
    /// Refuses a frame boundary outside the representable range.
    pub fn floor_frame(self, fps: Rational) -> Result<TimeCode, Refusal> {
        let (n, d) = self.frame_terms(fps)?;
        i64::try_from(n.div_euclid(d))
            .map(TimeCode)
            .map_err(|_| Refusal::arithmetic())
    }

    /// # Errors
    /// Refuses a frame boundary outside the representable range.
    pub fn ceil_frame(self, fps: Rational) -> Result<TimeCode, Refusal> {
        let (n, d) = self.frame_terms(fps)?;
        i64::try_from(-(-n).div_euclid(d))
            .map(TimeCode)
            .map_err(|_| Refusal::arithmetic())
    }

    fn frame_terms(self, fps: Rational) -> Result<(i128, i128), Refusal> {
        if !fps.is_valid() {
            return Err(Refusal::new(
                RefusalCode::UnsupportedRate,
                "The frame rate must be positive.",
            ));
        }
        Ok((
            i128::from(self.numerator) * i128::from(fps.numerator()),
            i128::from(self.denominator) * i128::from(fps.denominator()),
        ))
    }

    /// # Errors
    /// Refuses an unrepresentable time in seconds.
    pub fn from_frame(frame: TimeCode, fps: Rational) -> Result<Self, Refusal> {
        Self::wide(
            i128::from(frame.0) * i128::from(fps.denominator()),
            u128::from(fps.numerator()),
        )
    }

    /// # Errors
    /// Refuses a non-finite or unrepresentable floating-point time.
    pub fn from_float(value: f64) -> Result<Self, Refusal> {
        if !value.is_finite() {
            return Err(Refusal::arithmetic());
        }
        if value == 0.0 {
            return Ok(Self::ZERO);
        }
        let bits = value.to_bits();
        let exponent_bits = (bits >> 52) & 0x7ff;
        let mut mantissa = bits & ((1 << 52) - 1);
        let mut exponent = i32::try_from(exponent_bits).map_err(|_| Refusal::arithmetic())? - 1075;
        if exponent_bits == 0 {
            exponent += 1;
        } else {
            mantissa |= 1 << 52;
        }
        let zeros = mantissa.trailing_zeros();
        mantissa >>= zeros;
        exponent += i32::try_from(zeros).map_err(|_| Refusal::arithmetic())?;
        let (numerator, denominator) = if exponent < 0 {
            (
                i128::from(mantissa),
                1_u128
                    .checked_shl(exponent.unsigned_abs())
                    .ok_or_else(Refusal::arithmetic)?,
            )
        } else {
            let shift = exponent.unsigned_abs();
            if shift + (64 - mantissa.leading_zeros()) > 64 {
                return Err(Refusal::arithmetic());
            }
            (i128::from(mantissa) << shift, 1)
        };
        Self::wide(
            if value.is_sign_negative() {
                -numerator
            } else {
                numerator
            },
            denominator,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arithmetic_and_negative_grid_rounding_are_exact() {
        let t = Time::new(-7, 48).unwrap();
        let fps = Rational::new(24, 1).unwrap();
        assert_eq!(t.frame(fps).unwrap(), TimeCode(-3));
        assert_eq!(t.floor_frame(fps).unwrap(), TimeCode(-4));
        assert_eq!(t.ceil_frame(fps).unwrap(), TimeCode(-3));
        assert_eq!(Time::new(3, 8).unwrap(), Time::from_float(0.375).unwrap());
        assert_eq!(
            t.checked_div(Time::new(7, 12).unwrap()).unwrap(),
            Time::new(-1, 4).unwrap()
        );
        assert_eq!(
            Time::from_float(1e300).unwrap_err().code,
            RefusalCode::Unrepresentable
        );
        assert_eq!(
            Time::integer(i64::MAX)
                .checked_add(Time::ONE)
                .unwrap_err()
                .code,
            RefusalCode::Unrepresentable
        );
    }

    #[test]
    fn the_maximum_rate_at_the_maximum_speed_fits_core() {
        let rate = Time::integer(1_000).as_rate().unwrap();
        assert_eq!(
            kinewright_core::speed_scaled_fps(rate, 1_000).unwrap(),
            Rational::new(10_000, 1).unwrap()
        );
        assert_eq!(
            Time::integer(1_001).as_rate().unwrap_err().code,
            RefusalCode::UnsupportedRate
        );
    }
}
