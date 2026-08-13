use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::TimeCode;

const CURVE_SCALE: i128 = 1_000_000;

/// Interpolation applied from a keyframe to the next keyframe.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum KeyframeInterpolation {
    Hold,
    #[default]
    Linear,
    EaseIn,
    EaseOut,
    EaseInOut,
}

/// One exact, clip-local parameter value on an automation curve.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Keyframe {
    /// Exact frame in the owner's time base. Effect operations use clip-local
    /// frames; audio-bus effects use project frames.
    pub at: TimeCode,
    pub value: i64,
    #[serde(default)]
    #[schemars(default)]
    pub interpolation: KeyframeInterpolation,
}

/// A reusable fixed-point automation curve.
///
/// Evaluation is entirely integer based so preview, export, undo/redo, and
/// agent inspection agree at every project frame.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AutomationCurve {
    pub keyframes: Vec<Keyframe>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum AutomationCurveError {
    #[error("automation curve must contain at least one keyframe")]
    Empty,
    #[error("automation keyframe positions must be non-negative")]
    NegativePosition,
    #[error("automation keyframes must be strictly ordered by frame")]
    Unordered,
}

impl AutomationCurve {
    /// Validate the structural invariants shared by every automatable parameter.
    ///
    /// # Errors
    ///
    /// Returns an error for empty, negative, duplicate, or unsorted keyframes.
    pub fn validate(&self) -> Result<(), AutomationCurveError> {
        let Some(first) = self.keyframes.first() else {
            return Err(AutomationCurveError::Empty);
        };
        if first.at < TimeCode::ZERO {
            return Err(AutomationCurveError::NegativePosition);
        }
        for pair in self.keyframes.windows(2) {
            if pair[1].at < TimeCode::ZERO {
                return Err(AutomationCurveError::NegativePosition);
            }
            if pair[1].at <= pair[0].at {
                return Err(AutomationCurveError::Unordered);
            }
        }
        Ok(())
    }

    /// Evaluate at a clip-local frame, clamping outside the keyed interval.
    #[must_use]
    pub fn value_at(&self, at: TimeCode) -> Option<i64> {
        let first = *self.keyframes.first()?;
        if at <= first.at {
            return Some(first.value);
        }
        let last = *self.keyframes.last()?;
        if at >= last.at {
            return Some(last.value);
        }
        let pair = self
            .keyframes
            .windows(2)
            .find(|pair| at >= pair[0].at && at < pair[1].at)?;
        let start = pair[0];
        let end = pair[1];
        if start.interpolation == KeyframeInterpolation::Hold {
            return Some(start.value);
        }
        let span = i128::from(end.at.0 - start.at.0);
        let offset = i128::from(at.0 - start.at.0);
        let linear = offset.saturating_mul(CURVE_SCALE) / span;
        let eased = ease(linear, start.interpolation);
        let delta = i128::from(end.value) - i128::from(start.value);
        let interpolated = i128::from(start.value) + rounded_div(delta * eased, CURVE_SCALE);
        i64::try_from(interpolated).ok()
    }
}

fn ease(value: i128, interpolation: KeyframeInterpolation) -> i128 {
    match interpolation {
        KeyframeInterpolation::Hold => 0,
        KeyframeInterpolation::Linear => value,
        KeyframeInterpolation::EaseIn => value * value / CURVE_SCALE,
        KeyframeInterpolation::EaseOut => {
            let inverse = CURVE_SCALE - value;
            CURVE_SCALE - inverse * inverse / CURVE_SCALE
        }
        KeyframeInterpolation::EaseInOut => {
            let squared = value * value / CURVE_SCALE;
            squared * (3 * CURVE_SCALE - 2 * value) / CURVE_SCALE
        }
    }
}

fn rounded_div(numerator: i128, denominator: i128) -> i128 {
    if numerator >= 0 {
        (numerator + denominator / 2) / denominator
    } else {
        (numerator - denominator / 2) / denominator
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn curve(interpolation: KeyframeInterpolation) -> AutomationCurve {
        AutomationCurve {
            keyframes: vec![
                Keyframe {
                    at: TimeCode(10),
                    value: -100,
                    interpolation,
                },
                Keyframe {
                    at: TimeCode(20),
                    value: 100,
                    interpolation: KeyframeInterpolation::Linear,
                },
            ],
        }
    }

    #[test]
    fn clamps_before_and_after_keyed_interval() {
        let curve = curve(KeyframeInterpolation::Linear);
        assert_eq!(curve.value_at(TimeCode::ZERO), Some(-100));
        assert_eq!(curve.value_at(TimeCode(20)), Some(100));
        assert_eq!(curve.value_at(TimeCode(200)), Some(100));
    }

    #[test]
    fn evaluates_fixed_point_interpolations_at_exact_frames() {
        assert_eq!(
            curve(KeyframeInterpolation::Hold).value_at(TimeCode(15)),
            Some(-100)
        );
        assert_eq!(
            curve(KeyframeInterpolation::Linear).value_at(TimeCode(15)),
            Some(0)
        );
        assert_eq!(
            curve(KeyframeInterpolation::EaseIn).value_at(TimeCode(15)),
            Some(-50)
        );
        assert_eq!(
            curve(KeyframeInterpolation::EaseOut).value_at(TimeCode(15)),
            Some(50)
        );
        assert_eq!(
            curve(KeyframeInterpolation::EaseInOut).value_at(TimeCode(15)),
            Some(0)
        );
    }

    #[test]
    fn rejects_empty_negative_duplicate_and_unsorted_curves() {
        assert_eq!(
            AutomationCurve { keyframes: vec![] }.validate(),
            Err(AutomationCurveError::Empty)
        );
        let invalid = |positions: &[i64]| AutomationCurve {
            keyframes: positions
                .iter()
                .map(|at| Keyframe {
                    at: TimeCode(*at),
                    value: 0,
                    interpolation: KeyframeInterpolation::Linear,
                })
                .collect(),
        };
        assert_eq!(
            invalid(&[-1]).validate(),
            Err(AutomationCurveError::NegativePosition)
        );
        assert_eq!(
            invalid(&[1, 1]).validate(),
            Err(AutomationCurveError::Unordered)
        );
        assert_eq!(
            invalid(&[2, 1]).validate(),
            Err(AutomationCurveError::Unordered)
        );
    }
}
