use std::collections::BTreeMap;

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

/// AU4 §3.1: the value stepped discontinuously at the first sample of `at`.
///
/// `Some` when there is a key exactly at `at` whose *preceding* segment is
/// `Hold`. Integer only; the caller turns it into the forward declick.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HoldStep {
    /// The value held over the whole preceding segment.
    pub previous_value: i64,
    /// The frame of the key *following* the one at `at`, in the owner's own
    /// time base; `None` when the key at `at` is the last.
    pub next_key: Option<TimeCode>,
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

    /// AU4 §3.1: whether the value is flat from `at` to `at + 1` — outside the
    /// keyed interval, or inside a `Hold` segment.
    ///
    /// `value_at` clamps before the first key and at or after the last
    /// (:79-86), so both tails are flat; inside the keyed interval only a
    /// `Hold` segment is.
    #[must_use]
    pub fn holds_at(&self, at: TimeCode) -> bool {
        let Some(first) = self.keyframes.first() else {
            return true;
        };
        if at < first.at {
            return true;
        }
        let Some(last) = self.keyframes.last() else {
            return true;
        };
        if at >= last.at {
            return true;
        }
        self.keyframes
            .windows(2)
            .find(|pair| at >= pair[0].at && at < pair[1].at)
            .is_none_or(|pair| pair[0].interpolation == KeyframeInterpolation::Hold)
    }

    /// AU4 §3.1: see [`HoldStep`].
    #[must_use]
    pub fn hold_step_at(&self, at: TimeCode) -> Option<HoldStep> {
        let index = self.keyframes.iter().position(|key| key.at == at)?;
        let previous = self.keyframes.get(index.checked_sub(1)?)?;
        if previous.interpolation != KeyframeInterpolation::Hold {
            return None;
        }
        Some(HoldStep {
            previous_value: previous.value,
            next_key: self.keyframes.get(index + 1).map(|key| key.at),
        })
    }

    /// AU4 §2.3: the interpolation of the segment that contains `at`.
    ///
    /// The segment a boundary key inherits under `rebase_clip_curve` step 1:
    /// the key at or below `at` governs the shape to its right, so carrying
    /// its interpolation is what preserves that shape.
    pub(crate) fn segment_interpolation_at(&self, at: TimeCode) -> KeyframeInterpolation {
        let Some(first) = self.keyframes.first() else {
            return KeyframeInterpolation::default();
        };
        if at <= first.at {
            return first.interpolation;
        }
        self.keyframes
            .iter()
            .rev()
            .find(|key| key.at <= at)
            .map_or(first.interpolation, |key| key.interpolation)
    }
}

/// AU4 §2.3: move a clip-local curve to a new local origin and duration.
///
/// The value at the new boundary frame of a *shortening* edge is preserved
/// exactly; interior values are preserved to within one tenth-dB on `Linear`
/// segments and exactly on `Hold` segments, and are **re-shaped inside a
/// truncated eased segment** — an eased curve re-parameterised onto a shorter
/// span is a different curve. Exact eased truncation needs per-key tangents,
/// which are a named deferral (AU4 §1.3).
///
/// `delta_local` is **signed**: it is negative whenever the head is pulled out
/// (a left trim, a left slide, a left roll). In that case nothing is inserted
/// on the left, every key shifts right, and the newly exposed head is flat at
/// the first key's value because `value_at` clamps before the first key.
///
/// Precondition: `delta_local` and `new_duration` are project-frame values
/// bounded by `document.duration` (every in-tree caller derives them from
/// validated `timeline_start`s); the two subtractions below saturate so an
/// out-of-tree caller at the `i64` extremes cannot overflow.
#[must_use]
pub fn rebase_clip_curve(
    curve: &AutomationCurve,
    delta_local: TimeCode,
    new_duration: TimeCode,
) -> AutomationCurve {
    if curve.keyframes.is_empty() {
        return curve.clone();
    }
    // A clip never has a non-positive project duration (`ZeroProjectDuration`),
    // but the reduction is defined anyway so the function is total.
    if new_duration <= TimeCode::ZERO {
        return single_key_at(curve, TimeCode::ZERO, delta_local);
    }
    let mut keys: BTreeMap<i64, Keyframe> = BTreeMap::new();
    // Step 2 and 3: shift by `-delta_local`, drop outside `0..new_duration`.
    for key in &curve.keyframes {
        let at = key.at.0.saturating_sub(delta_local.0);
        if (0..new_duration.0).contains(&at) {
            keys.insert(
                at,
                Keyframe {
                    at: TimeCode(at),
                    ..*key
                },
            );
        }
    }
    // Step 1: the boundary keys, valued from the *old* curve. Inserted last so
    // step 4's last-wins dedupe prefers them; rule 14 proves the two agree
    // whenever they collide, so the order cannot pick a wrong value.
    if curve.keyframes.iter().any(|key| key.at < delta_local) {
        insert_boundary_key(&mut keys, curve, TimeCode::ZERO, delta_local);
    }
    let right_source = TimeCode(
        delta_local
            .0
            .saturating_add(new_duration.0)
            .saturating_sub(1),
    );
    if curve
        .keyframes
        .iter()
        .any(|key| key.at.0 - delta_local.0 >= new_duration.0)
    {
        insert_boundary_key(&mut keys, curve, TimeCode(new_duration.0 - 1), right_source);
    }
    // Step 5 is defensive: with a non-empty curve and `new_duration > 0`,
    // every dropped key fires one of the two boundary inserts above, so the
    // all-dropped case is already a single boundary key and this arm is
    // unreachable. It stays so the "never falls back to the static scalar"
    // rule is enforced here too, not only implied by the arms above.
    debug_assert!(
        !keys.is_empty(),
        "a dropped key always inserts a boundary key"
    );
    if keys.is_empty() {
        return single_key_at(curve, TimeCode::ZERO, delta_local);
    }
    AutomationCurve {
        keyframes: keys.into_values().collect(),
    }
}

/// AU4 §2.3: clamp a project-frame curve into a shortened project without
/// changing the value audible at the new last frame.
///
/// A curve already inside `new_duration` is returned unchanged, so no document
/// that did not shorten changes. `new_duration <= 0` reduces the curve to a
/// single key at frame 0 valued `value_at(0)` — a one-key constant curve on an
/// empty timeline is a legal document (AU4 §2.3 rule 17).
#[must_use]
pub fn clamp_project_curve(curve: &AutomationCurve, new_duration: TimeCode) -> AutomationCurve {
    if curve.keyframes.is_empty() {
        return curve.clone();
    }
    if new_duration <= TimeCode::ZERO {
        return single_key_at(curve, TimeCode::ZERO, TimeCode::ZERO);
    }
    if curve.keyframes.iter().all(|key| key.at.0 < new_duration.0) {
        return curve.clone();
    }
    let boundary = TimeCode(new_duration.0 - 1);
    let mut keys: BTreeMap<i64, Keyframe> = BTreeMap::new();
    for key in &curve.keyframes {
        if key.at.0 < new_duration.0 {
            keys.insert(key.at.0, *key);
        }
    }
    insert_boundary_key(&mut keys, curve, boundary, boundary);
    AutomationCurve {
        keyframes: keys.into_values().collect(),
    }
}

/// The curve reduced to one key at `at`, valued as the old curve read at
/// `source`.
fn single_key_at(curve: &AutomationCurve, at: TimeCode, source: TimeCode) -> AutomationCurve {
    AutomationCurve {
        keyframes: vec![boundary_key(curve, at, source)],
    }
}

fn insert_boundary_key(
    keys: &mut BTreeMap<i64, Keyframe>,
    curve: &AutomationCurve,
    at: TimeCode,
    source: TimeCode,
) {
    keys.insert(at.0, boundary_key(curve, at, source));
}

fn boundary_key(curve: &AutomationCurve, at: TimeCode, source: TimeCode) -> Keyframe {
    Keyframe {
        at,
        // `value_at` is `None` only on i64 overflow, which falls back to the
        // first key rather than inventing a bound the editor never wrote.
        value: curve
            .value_at(source)
            .unwrap_or_else(|| curve.keyframes.first().map_or(0, |key| key.value)),
        interpolation: curve.segment_interpolation_at(source),
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

    /// AU4 §3.1: `holds_at` is flat outside the keyed interval and inside a
    /// `Hold` segment, and nowhere else.
    #[test]
    fn holds_at_is_true_outside_the_keyed_interval_and_inside_a_hold_segment() {
        let linear = curve(KeyframeInterpolation::Linear);
        // Before the first key and at or after the last, `value_at` clamps.
        for at in [-5, 0, 9, 20, 21, 200] {
            assert!(linear.holds_at(TimeCode(at)), "flat at {at}");
        }
        // Inside a `Linear` segment it is not flat.
        for at in 10..20 {
            assert!(!linear.holds_at(TimeCode(at)), "sloped at {at}");
        }
        let held = curve(KeyframeInterpolation::Hold);
        for at in 10..20 {
            assert!(
                held.holds_at(TimeCode(at)),
                "a Hold segment is flat at {at}"
            );
            assert_eq!(held.value_at(TimeCode(at)), Some(-100));
        }
        // Rule 41.4 is what makes the segment flat: `holds_at` stays true at
        // the segment's last frame, so the value steps at the *next* key's
        // first sample rather than ramping through frame 19.
        assert!(held.holds_at(TimeCode(19)));
        assert_eq!(held.value_at(TimeCode(20)), Some(100));
        // An empty curve has no value to change.
        assert!(AutomationCurve { keyframes: vec![] }.holds_at(TimeCode(3)));
    }

    /// AU4 §3.1: `hold_step_at` is `Some` exactly at a key whose *preceding*
    /// segment is `Hold`.
    #[test]
    fn hold_step_at_reports_the_held_value_and_the_following_key() {
        let stepped = AutomationCurve {
            keyframes: vec![
                Keyframe {
                    at: TimeCode(10),
                    value: -300,
                    interpolation: KeyframeInterpolation::Hold,
                },
                Keyframe {
                    at: TimeCode(100),
                    value: 0,
                    interpolation: KeyframeInterpolation::Linear,
                },
                Keyframe {
                    at: TimeCode(150),
                    value: -100,
                    interpolation: KeyframeInterpolation::Hold,
                },
                Keyframe {
                    at: TimeCode(200),
                    value: 0,
                    interpolation: KeyframeInterpolation::Linear,
                },
            ],
        };
        // The first key has no preceding segment.
        assert_eq!(stepped.hold_step_at(TimeCode(10)), None);
        // Not at a key at all.
        assert_eq!(stepped.hold_step_at(TimeCode(50)), None);
        // The step at 100 is preceded by a `Hold` segment.
        assert_eq!(
            stepped.hold_step_at(TimeCode(100)),
            Some(HoldStep {
                previous_value: -300,
                next_key: Some(TimeCode(150)),
            })
        );
        // The step at 150 is preceded by a `Linear` segment, so it is not one.
        assert_eq!(stepped.hold_step_at(TimeCode(150)), None);
        // The last key reports no following key.
        assert_eq!(
            stepped.hold_step_at(TimeCode(200)),
            Some(HoldStep {
                previous_value: -100,
                next_key: None,
            })
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
