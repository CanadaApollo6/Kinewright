use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::TimeCode;

const CURVE_SCALE: i64 = 1_000_000;

/// MO1 R31: the proven value bound. Every green H1–H3/H6 cell proves its
/// property for key values within `±PROVEN_VALUE_BOUND`; the sweep test below
/// pins every non-hold descriptor range plus the clip/track/bus/master curve
/// ranges inside it, so every validated document evaluates inside the proof.
pub const PROVEN_VALUE_BOUND: i64 = 1_000_000;

/// MO1 review F3: the keep-outside position bound. `validate_ordered`
/// refuses keys with `|at|` past 2^40 frames (~35 million years at 30 fps —
/// inexhaustible editorially) so spans stay within 2^41 and
/// `offset × CURVE_SCALE` tops out near 2.2e18, far inside `i64` with no
/// saturation, and `shift_keys_keep_outside` can never saturate-collapse
/// two validated keys onto one frame.
pub const MAX_KEY_FRAME_OFFSET: i64 = 1 << 40;

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
    /// MO1 R6: reserved Bezier incoming tangent, never evaluated here.
    /// Serde-defaulted to 0 and skipped when 0, so existing documents are
    /// byte-identical; validation accepts any value; survival preserves
    /// verbatim via the existing `..*key` updates and the §8 kernels.
    #[serde(default, skip_serializing_if = "i64_is_zero")]
    #[schemars(default)]
    pub tangent_in: i64,
    /// MO1 R6: reserved Bezier outgoing tangent, never evaluated here.
    /// See [`Keyframe::tangent_in`].
    #[serde(default, skip_serializing_if = "i64_is_zero")]
    #[schemars(default)]
    pub tangent_out: i64,
}

// Serde's `skip_serializing_if` callbacks receive references to the fields.
#[allow(clippy::trivially_copy_pass_by_ref)]
const fn i64_is_zero(value: &i64) -> bool {
    *value == 0
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
    #[error("automation keyframe positions must be within ±2^40 frames")]
    PositionOutOfBounds,
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

    /// MO1 R13: ordered-only validation for keep-outside owners — non-empty
    /// and strictly ordered, sign-agnostic (negative `at` is legal after a
    /// trim-in), with `|at|` bounded by [`MAX_KEY_FRAME_OFFSET`] (review F3).
    /// Audio owners keep strict [`validate`](Self::validate).
    ///
    /// # Errors
    ///
    /// Returns an error for empty, out-of-bounds, duplicate, or unsorted
    /// keyframes.
    pub fn validate_ordered(&self) -> Result<(), AutomationCurveError> {
        if self.keyframes.is_empty() {
            return Err(AutomationCurveError::Empty);
        }
        if self
            .keyframes
            .iter()
            .any(|keyframe| keyframe.at.0.unsigned_abs() > MAX_KEY_FRAME_OFFSET as u64)
        {
            return Err(AutomationCurveError::PositionOutOfBounds);
        }
        if self
            .keyframes
            .windows(2)
            .any(|pair| pair[1].at <= pair[0].at)
        {
            return Err(AutomationCurveError::Unordered);
        }
        Ok(())
    }

    /// Evaluate at a clip-local frame, clamping outside the keyed interval.
    ///
    /// MO1 R31: thin wrapper over the slice-level [`value_at_keys`] kernel —
    /// the shipped maths IS the proved kernel, not a shadow of it.
    #[must_use]
    pub fn value_at(&self, at: TimeCode) -> Option<i64> {
        value_at_keys(&self.keyframes, at.0)
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
        value: curve
            .value_at(source)
            .unwrap_or_else(|| curve.keyframes.first().map_or(0, |key| key.value)),
        interpolation: curve.segment_interpolation_at(source),
        tangent_in: 0,
        tangent_out: 0,
    }
}

/// MO1 R31: slice-level evaluation kernel — the proved unit behind
/// [`AutomationCurve::value_at`], which is a thin wrapper over this.
///
/// Same shape as the pre-MO1 `i128` implementation: clamp outside the keyed
/// interval, linear window scan, `Hold` short-circuit, `offset * SCALE / span`,
/// eased interpolation with round-half-away-from-zero. Behaviour-preserving
/// for validated documents: it differs from the old `i128` maths only if a
/// key-value difference exceeds ~9.2e12, unreachable because keep-outside
/// spans are bounded by `2 × MAX_KEY_FRAME_OFFSET` (`validate_ordered`),
/// audio spans by document duration, and every non-hold descriptor range fits
/// `±PROVEN_VALUE_BOUND` (sweep test below).
///
/// The `Hold` arm returns before any subtraction, so hold-only giants
/// (`LUT_ASSET_ID_DESCRIPTOR_MAX` 2^53−1) never reach value arithmetic.
/// Outside the proven range the kernel uses checked arithmetic returning
/// `None`, matching the old `try_from().ok()` arm. `None` only for an empty
/// curve or a checked-overflow anywhere past the `Hold` arm.
#[must_use]
pub fn value_at_keys(keys: &[Keyframe], at: i64) -> Option<i64> {
    let first = *keys.first()?;
    if at <= first.at.0 {
        return Some(first.value);
    }
    let last = *keys.last()?;
    if at >= last.at.0 {
        return Some(last.value);
    }
    let mut index = 0;
    while index + 1 < keys.len() {
        let start = keys[index];
        let end = keys[index + 1];
        if at >= start.at.0 && at < end.at.0 {
            if start.interpolation == KeyframeInterpolation::Hold {
                return Some(start.value);
            }
            let span = end.at.0.checked_sub(start.at.0)?;
            let offset = at.checked_sub(start.at.0)?;
            // `span > 0`: the window guard gives `start.at <= at < end.at`,
            // hence `start.at < end.at`. The saturating first multiply is kept
            // from the `i128` implementation.
            let linear = offset.saturating_mul(CURVE_SCALE) / span;
            let eased = ease_kernel(linear, start.interpolation);
            let delta = end.value.checked_sub(start.value)?;
            let product = delta.checked_mul(eased)?;
            let adjustment = rounded_div_kernel(product, CURVE_SCALE)?;
            return start.value.checked_add(adjustment);
        }
        index += 1;
    }
    None
}

/// MO1 R31: slice-level easing kernel over `0..=CURVE_SCALE`, proved through
/// H1 with all five kinds symbolic. Plain `i64` arithmetic: at the top of the
/// domain (`t == 1_000_000`) `t * t` is at most `1e12` and the `EaseInOut`
/// product `sq * (3S − 2t)` at most `3e12` — six orders below `i64::MAX` —
/// and every green cell's passing overflow checks prove it.
#[must_use]
pub fn ease_kernel(value: i64, interpolation: KeyframeInterpolation) -> i64 {
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

/// MO1 R31: round-half-away-from-zero division for the evaluation kernel.
/// Checked: returns `None` when the rounding bias overflows, matching the
/// kernel's checked-`None` fallback.
fn rounded_div_kernel(numerator: i64, denominator: i64) -> Option<i64> {
    if numerator >= 0 {
        numerator
            .checked_add(denominator / 2)
            .map(|biased| biased / denominator)
    } else {
        numerator
            .checked_sub(denominator / 2)
            .map(|biased| biased / denominator)
    }
}

/// MO1 R11/R31: slice-level keep-outside shift — the proved unit behind
/// [`rebase_clip_curve_keep_outside`]. Every key moves by `-delta`;
/// nothing is dropped, so shifting by `delta` then `-delta` is identity (H4).
/// Saturating so an out-of-tree caller at the `i64` extremes cannot overflow;
/// in-tree deltas are bounded by document duration and never saturate.
/// `out` must hold at least `keys.len()` entries. Tangents ride via `..*`.
pub fn shift_keys_keep_outside(keys: &[Keyframe], delta: i64, out: &mut [Keyframe]) {
    let mut index = 0;
    while index < keys.len() {
        out[index] = Keyframe {
            at: TimeCode(keys[index].at.0.saturating_sub(delta)),
            ..keys[index]
        };
        index += 1;
    }
}

/// MO1 R11: shift every key by `-delta_local` (signed, saturating), drop
/// nothing, insert no boundary key. `delta_local` keeps its AU4 meaning
/// (`new_timeline_start − old_timeline_start`).
#[must_use]
pub fn rebase_clip_curve_keep_outside(
    curve: &AutomationCurve,
    delta_local: TimeCode,
) -> AutomationCurve {
    if curve.keyframes.is_empty() {
        return curve.clone();
    }
    let mut shifted = Vec::with_capacity(curve.keyframes.len());
    shifted.resize(curve.keyframes.len(), curve.keyframes[0]);
    shift_keys_keep_outside(&curve.keyframes, delta_local.0, &mut shifted);
    AutomationCurve { keyframes: shifted }
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
                    tangent_in: 0,
                    tangent_out: 0,
                },
                Keyframe {
                    at: TimeCode(20),
                    value: 100,
                    interpolation: KeyframeInterpolation::Linear,
                    tangent_in: 0,
                    tangent_out: 0,
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
                    tangent_in: 0,
                    tangent_out: 0,
                },
                Keyframe {
                    at: TimeCode(100),
                    value: 0,
                    interpolation: KeyframeInterpolation::Linear,
                    tangent_in: 0,
                    tangent_out: 0,
                },
                Keyframe {
                    at: TimeCode(150),
                    value: -100,
                    interpolation: KeyframeInterpolation::Hold,
                    tangent_in: 0,
                    tangent_out: 0,
                },
                Keyframe {
                    at: TimeCode(200),
                    value: 0,
                    interpolation: KeyframeInterpolation::Linear,
                    tangent_in: 0,
                    tangent_out: 0,
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

    /// MO1 R13: ordered-only validation accepts negative positions but
    /// still rejects empty and unordered curves.
    #[test]
    fn validate_ordered_is_sign_agnostic_but_still_ordered() {
        let ordered = |positions: &[i64]| AutomationCurve {
            keyframes: positions
                .iter()
                .map(|at| Keyframe {
                    at: TimeCode(*at),
                    value: 0,
                    interpolation: KeyframeInterpolation::Linear,
                    tangent_in: 0,
                    tangent_out: 0,
                })
                .collect(),
        };
        assert_eq!(ordered(&[-20, -5, 0, 30]).validate_ordered(), Ok(()));
        assert_eq!(
            ordered(&[]).validate_ordered(),
            Err(AutomationCurveError::Empty)
        );
        assert_eq!(
            ordered(&[1, 1]).validate_ordered(),
            Err(AutomationCurveError::Unordered)
        );
        assert_eq!(
            ordered(&[2, 1]).validate_ordered(),
            Err(AutomationCurveError::Unordered)
        );
        assert_eq!(
            ordered(&[-5, -20]).validate_ordered(),
            Err(AutomationCurveError::Unordered)
        );
        // Strict `validate` still refuses the negative curve.
        assert_eq!(
            ordered(&[-20, -5, 0, 30]).validate(),
            Err(AutomationCurveError::NegativePosition)
        );
    }

    /// MO1 review F3: ordered-only validation still bounds `|at|` — keys
    /// past `±MAX_KEY_FRAME_OFFSET` are refused, so spans stay far inside
    /// `i64 × CURVE_SCALE` and shifts never saturate-collapse.
    #[test]
    fn validate_ordered_bounds_keyframe_positions() {
        let bound = super::MAX_KEY_FRAME_OFFSET;
        let ordered = |positions: &[i64]| AutomationCurve {
            keyframes: positions
                .iter()
                .map(|at| Keyframe {
                    at: TimeCode(*at),
                    value: 0,
                    interpolation: KeyframeInterpolation::Linear,
                    tangent_in: 0,
                    tangent_out: 0,
                })
                .collect(),
        };
        assert_eq!(ordered(&[-bound, bound]).validate_ordered(), Ok(()));
        assert_eq!(
            ordered(&[bound + 1]).validate_ordered(),
            Err(AutomationCurveError::PositionOutOfBounds)
        );
        assert_eq!(
            ordered(&[-bound - 1]).validate_ordered(),
            Err(AutomationCurveError::PositionOutOfBounds)
        );
        assert_eq!(
            ordered(&[i64::MIN]).validate_ordered(),
            Err(AutomationCurveError::PositionOutOfBounds)
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
                    tangent_in: 0,
                    tangent_out: 0,
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

    /// MO1 R31: every non-hold descriptor range plus the clip/track/bus/master
    /// curve ranges fit `±PROVEN_VALUE_BOUND`, so every validated document
    /// evaluates inside the Kani proof. Compile-forced: a new descriptor row
    /// outside the bound fails here until the proof is re-scoped.
    #[test]
    fn descriptor_and_curve_ranges_fit_proven_bound() {
        for descriptor in crate::EFFECT_DESCRIPTORS {
            for parameter in descriptor.parameters {
                if crate::is_hold_only_parameter(descriptor.name, parameter.name) {
                    continue;
                }
                assert!(
                    -PROVEN_VALUE_BOUND <= parameter.min
                        && parameter.min <= parameter.max
                        && parameter.max <= PROVEN_VALUE_BOUND,
                    "descriptor {}.{} range {}..={} exceeds ±{PROVEN_VALUE_BOUND}",
                    descriptor.name,
                    parameter.name,
                    parameter.min,
                    parameter.max
                );
            }
        }
        // Clip gain envelope (`CLIP_GAIN_MIN..=CLIP_GAIN_MAX`, shared with
        // `validate_clip_gain_curve` so the two cannot drift).
        for bound in [
            i64::from(crate::CLIP_GAIN_MIN),
            i64::from(crate::CLIP_GAIN_MAX),
        ] {
            assert!(bound.abs() <= PROVEN_VALUE_BOUND, "clip gain {bound}");
        }
        // Track gain/pan, bus/master gain: the `model.rs` consts.
        for bound in [
            i64::from(crate::TRACK_MIX_GAIN_MIN),
            i64::from(crate::TRACK_MIX_GAIN_MAX),
            i64::from(crate::TRACK_MIX_PAN_MIN),
            i64::from(crate::TRACK_MIX_PAN_MAX),
            i64::from(crate::AUDIO_BUS_GAIN_MIN),
            i64::from(crate::AUDIO_BUS_GAIN_MAX),
            i64::from(crate::AUDIO_MASTER_GAIN_MIN),
            i64::from(crate::AUDIO_MASTER_GAIN_MAX),
        ] {
            assert!(bound.abs() <= PROVEN_VALUE_BOUND, "mix gain/pan {bound}");
        }
    }

    /// MO1 R31: the `Hold` arm returns before any subtraction, so hold-only
    /// giants never reach value arithmetic; non-hold giants return `None`
    /// through checked arithmetic, matching the old `try_from().ok()`.
    #[test]
    fn hold_giants_read_back_and_non_hold_giants_return_none() {
        // `LUT_ASSET_ID_DESCRIPTOR_MAX` is 2^53−1; a `Hold` curve over it
        // reads back exactly, never touching the value maths.
        let giant = 9_007_199_254_740_991_i64;
        let held = AutomationCurve {
            keyframes: vec![
                Keyframe {
                    at: TimeCode(0),
                    value: giant,
                    interpolation: KeyframeInterpolation::Hold,
                    tangent_in: 0,
                    tangent_out: 0,
                },
                Keyframe {
                    at: TimeCode(10),
                    value: 0,
                    interpolation: KeyframeInterpolation::Linear,
                    tangent_in: 0,
                    tangent_out: 0,
                },
            ],
        };
        assert_eq!(held.value_at(TimeCode(5)), Some(giant));
        assert_eq!(held.value_at(TimeCode(0)), Some(giant));
        // A `Linear` curve whose key-value difference overflows `i64`
        // returns `None` instead of panicking or wrapping.
        let overflowing = AutomationCurve {
            keyframes: vec![
                Keyframe {
                    at: TimeCode(0),
                    value: i64::MAX,
                    interpolation: KeyframeInterpolation::Linear,
                    tangent_in: 0,
                    tangent_out: 0,
                },
                Keyframe {
                    at: TimeCode(10),
                    value: i64::MIN,
                    interpolation: KeyframeInterpolation::Linear,
                    tangent_in: 0,
                    tangent_out: 0,
                },
            ],
        };
        assert_eq!(overflowing.value_at(TimeCode(5)), None);
        // A `Linear` curve whose `delta * eased` overflows `i64` near the
        // segment end (delta 1.8e13, eased 9e5 at frame 9 → 1.62e19)
        // returns `None`; at the midpoint (eased 5e5 → 9e18, fits) it
        // still reads back exactly.
        let wide = AutomationCurve {
            keyframes: vec![
                Keyframe {
                    at: TimeCode(0),
                    value: -9_000_000_000_000,
                    interpolation: KeyframeInterpolation::Linear,
                    tangent_in: 0,
                    tangent_out: 0,
                },
                Keyframe {
                    at: TimeCode(10),
                    value: 9_000_000_000_000,
                    interpolation: KeyframeInterpolation::Linear,
                    tangent_in: 0,
                    tangent_out: 0,
                },
            ],
        };
        assert_eq!(wide.value_at(TimeCode(5)), Some(0));
        assert_eq!(wide.value_at(TimeCode(9)), None);
    }

    /// MO1 review F4 (mutation 2): the `Hold` arm returns before any
    /// subtraction — a held `i64::MAX` reads back over a segment whose
    /// value delta (`i64::MIN − i64::MAX`) would overflow, which the
    /// checked path turns into `None` instead. Removing the arm flips
    /// this `Some` to `None`.
    #[test]
    fn hold_arm_reads_back_before_overflowable_subtraction() {
        let held = AutomationCurve {
            keyframes: vec![
                Keyframe {
                    at: TimeCode(0),
                    value: i64::MAX,
                    interpolation: KeyframeInterpolation::Hold,
                    tangent_in: 0,
                    tangent_out: 0,
                },
                Keyframe {
                    at: TimeCode(10),
                    value: i64::MIN,
                    interpolation: KeyframeInterpolation::Linear,
                    tangent_in: 0,
                    tangent_out: 0,
                },
            ],
        };
        assert_eq!(held.value_at(TimeCode(5)), Some(i64::MAX));
    }

    /// MO1 R31: the method IS the kernel — `value_at` agrees with
    /// `value_at_keys` on a reference curve covering all five easings plus
    /// both clamp tails, and the `i64` kernel agrees with a reimplemented
    /// `i128` reference on an in-bounds grid.
    #[test]
    fn method_matches_kernel_and_i64_matches_i128_in_bounds() {
        let reference = AutomationCurve {
            keyframes: vec![
                Keyframe {
                    at: TimeCode(0),
                    value: -500_000,
                    interpolation: KeyframeInterpolation::Hold,
                    tangent_in: 0,
                    tangent_out: 0,
                },
                Keyframe {
                    at: TimeCode(10),
                    value: 200_000,
                    interpolation: KeyframeInterpolation::Linear,
                    tangent_in: 0,
                    tangent_out: 0,
                },
                Keyframe {
                    at: TimeCode(20),
                    value: -300_000,
                    interpolation: KeyframeInterpolation::EaseIn,
                    tangent_in: 0,
                    tangent_out: 0,
                },
                Keyframe {
                    at: TimeCode(30),
                    value: 400_000,
                    interpolation: KeyframeInterpolation::EaseOut,
                    tangent_in: 0,
                    tangent_out: 0,
                },
                Keyframe {
                    at: TimeCode(40),
                    value: -100_000,
                    interpolation: KeyframeInterpolation::EaseInOut,
                    tangent_in: 0,
                    tangent_out: 0,
                },
                Keyframe {
                    at: TimeCode(50),
                    value: 600_000,
                    interpolation: KeyframeInterpolation::Linear,
                    tangent_in: 0,
                    tangent_out: 0,
                },
            ],
        };
        for at in -8..=60 {
            let method = reference.value_at(TimeCode(at));
            let kernel = super::value_at_keys(&reference.keyframes, at);
            assert_eq!(method, kernel, "method≡kernel at {at}");
            let wide = value_at_i128_reference(&reference.keyframes, at);
            assert_eq!(method, wide, "i64≡i128 at {at}");
        }
        // Grid over values × kinds at a fixed span: every in-bounds cell
        // agrees with the `i128` reference.
        for kind in [
            KeyframeInterpolation::Hold,
            KeyframeInterpolation::Linear,
            KeyframeInterpolation::EaseIn,
            KeyframeInterpolation::EaseOut,
            KeyframeInterpolation::EaseInOut,
        ] {
            for (start_value, end_value) in [
                (-1_000_000, 1_000_000),
                (1_000_000, -1_000_000),
                (-100, 100),
                (0, 0),
            ] {
                let keys = vec![
                    Keyframe {
                        at: TimeCode(0),
                        value: start_value,
                        interpolation: kind,
                        tangent_in: 0,
                        tangent_out: 0,
                    },
                    Keyframe {
                        at: TimeCode(8),
                        value: end_value,
                        interpolation: KeyframeInterpolation::Linear,
                        tangent_in: 0,
                        tangent_out: 0,
                    },
                ];
                for at in 0..=8 {
                    assert_eq!(
                        super::value_at_keys(&keys, at),
                        value_at_i128_reference(&keys, at),
                        "i64≡i128 for {kind:?} {start_value}→{end_value} at {at}"
                    );
                }
            }
        }
    }

    /// Pre-MO1 `i128` maths, reimplemented for the migration differential.
    /// Returns `None` only for an empty curve or a final value outside `i64`.
    fn value_at_i128_reference(keys: &[Keyframe], at: i64) -> Option<i64> {
        const SCALE: i128 = 1_000_000;
        let first = *keys.first()?;
        if at <= first.at.0 {
            return Some(first.value);
        }
        let last = *keys.last()?;
        if at >= last.at.0 {
            return Some(last.value);
        }
        let mut index = 0;
        while index + 1 < keys.len() {
            let start = keys[index];
            let end = keys[index + 1];
            if at >= start.at.0 && at < end.at.0 {
                if start.interpolation == KeyframeInterpolation::Hold {
                    return Some(start.value);
                }
                let span = i128::from(end.at.0 - start.at.0);
                let offset = i128::from(at - start.at.0);
                let linear = offset.saturating_mul(SCALE) / span;
                let eased = match start.interpolation {
                    KeyframeInterpolation::Hold => 0,
                    KeyframeInterpolation::Linear => linear,
                    KeyframeInterpolation::EaseIn => linear * linear / SCALE,
                    KeyframeInterpolation::EaseOut => {
                        let inverse = SCALE - linear;
                        SCALE - inverse * inverse / SCALE
                    }
                    KeyframeInterpolation::EaseInOut => {
                        let squared = linear * linear / SCALE;
                        squared * (3 * SCALE - 2 * linear) / SCALE
                    }
                };
                let delta = i128::from(end.value) - i128::from(start.value);
                let numerator = delta * eased;
                let adjustment = if numerator >= 0 {
                    (numerator + SCALE / 2) / SCALE
                } else {
                    (numerator - SCALE / 2) / SCALE
                };
                return i64::try_from(i128::from(start.value) + adjustment).ok();
            }
            index += 1;
        }
        None
    }

    /// MO1 R6: nonzero tangents survive serde; zero tangents serialize
    /// byte-identically to pre-MO1 documents (fields skipped when 0).
    #[test]
    fn tangents_round_trip_and_legacy_docs_stay_byte_identical() {
        let keyed = Keyframe {
            at: TimeCode(5),
            value: 100,
            interpolation: KeyframeInterpolation::Linear,
            tangent_in: -250,
            tangent_out: 750,
        };
        let json = serde_json::to_string(&keyed).unwrap();
        assert!(
            json.contains("tangent_in"),
            "nonzero tangents persist: {json}"
        );
        assert!(
            json.contains("tangent_out"),
            "nonzero tangents persist: {json}"
        );
        assert_eq!(serde_json::from_str::<Keyframe>(&json).unwrap(), keyed);
        // Zero tangents omit both fields: byte-identical to pre-MO1.
        let plain = Keyframe {
            tangent_in: 0,
            tangent_out: 0,
            ..keyed
        };
        let plain_json = serde_json::to_string(&plain).unwrap();
        assert_eq!(
            plain_json,
            r#"{"at":5,"value":100,"interpolation":"linear"}"#
        );
        // Legacy documents (no tangent fields) read back with zero tangents.
        assert_eq!(
            serde_json::from_str::<Keyframe>(&plain_json).unwrap(),
            plain
        );
    }

    /// MO1 R6: evaluation ignores both tangent fields for every
    /// interpolation kind — no `KeyframeInterpolation` variant was added, so
    /// every existing match stays exhaustive and behaviour is unchanged.
    #[test]
    fn nonzero_tangents_change_no_evaluation() {
        for kind in [
            KeyframeInterpolation::Hold,
            KeyframeInterpolation::Linear,
            KeyframeInterpolation::EaseIn,
            KeyframeInterpolation::EaseOut,
            KeyframeInterpolation::EaseInOut,
        ] {
            let plain = AutomationCurve {
                keyframes: vec![
                    Keyframe {
                        at: TimeCode(0),
                        value: -500_000,
                        interpolation: kind,
                        tangent_in: 0,
                        tangent_out: 0,
                    },
                    Keyframe {
                        at: TimeCode(10),
                        value: 300_000,
                        interpolation: KeyframeInterpolation::Linear,
                        tangent_in: 0,
                        tangent_out: 0,
                    },
                ],
            };
            let keyed = AutomationCurve {
                keyframes: vec![
                    Keyframe {
                        tangent_in: -1_000_000,
                        tangent_out: 1_000_000,
                        ..plain.keyframes[0]
                    },
                    Keyframe {
                        tangent_in: 42,
                        tangent_out: -42,
                        ..plain.keyframes[1]
                    },
                ],
            };
            for at in -5..=15 {
                assert_eq!(
                    keyed.value_at(TimeCode(at)),
                    plain.value_at(TimeCode(at)),
                    "tangents change no value_at for {kind:?} at {at}"
                );
            }
        }
    }

    /// MO1 R6: survival preserves tangents verbatim — they ride the existing
    /// `..*key` updates and the §8 shift kernel carries them too.
    #[test]
    fn keep_outside_shift_preserves_tangents_verbatim() {
        let curve = AutomationCurve {
            keyframes: vec![
                Keyframe {
                    at: TimeCode(10),
                    value: -100,
                    interpolation: KeyframeInterpolation::EaseInOut,
                    tangent_in: -7,
                    tangent_out: 9,
                },
                Keyframe {
                    at: TimeCode(20),
                    value: 100,
                    interpolation: KeyframeInterpolation::Linear,
                    tangent_in: 3,
                    tangent_out: -3,
                },
            ],
        };
        let shifted = super::rebase_clip_curve_keep_outside(&curve, TimeCode(4));
        assert_eq!(shifted.keyframes.len(), 2);
        assert_eq!(shifted.keyframes[0].at, TimeCode(6));
        assert_eq!(shifted.keyframes[1].at, TimeCode(16));
        assert_eq!(shifted.keyframes[0].tangent_in, -7);
        assert_eq!(shifted.keyframes[0].tangent_out, 9);
        assert_eq!(shifted.keyframes[1].tangent_in, 3);
        assert_eq!(shifted.keyframes[1].tangent_out, -3);
    }

    #[test]
    fn translation_equivariance_holds_on_reference_curve() {
        let keys = vec![
            Keyframe {
                at: TimeCode(0),
                value: -500_000,
                interpolation: KeyframeInterpolation::EaseInOut,
                tangent_in: 0,
                tangent_out: 0,
            },
            Keyframe {
                at: TimeCode(10),
                value: 200_000,
                interpolation: KeyframeInterpolation::EaseIn,
                tangent_in: 0,
                tangent_out: 0,
            },
            Keyframe {
                at: TimeCode(25),
                value: 800_000,
                interpolation: KeyframeInterpolation::EaseOut,
                tangent_in: 0,
                tangent_out: 0,
            },
            Keyframe {
                at: TimeCode(40),
                value: -100_000,
                interpolation: KeyframeInterpolation::Linear,
                tangent_in: 0,
                tangent_out: 0,
            },
        ];
        let orig = AutomationCurve { keyframes: keys };
        for delta in [-32, -20, -1, 0, 1, 7, 32] {
            let shifted = super::rebase_clip_curve_keep_outside(&orig, TimeCode(delta));
            for at in [-8, -1, 0, 5, 10, 17, 25, 33, 40, 41, 64] {
                let expected = orig.value_at(TimeCode(at));
                let actual = shifted.value_at(TimeCode(at - delta));
                assert!(expected.is_some(), "in-bounds reads Some at {at}");
                assert_eq!(actual, expected, "equivariance at {at} shifted by {delta}");
            }
        }
    }
}

#[cfg(test)]
mod mo1_proptest {
    use proptest::prelude::*;

    use super::{AutomationCurve, Keyframe, KeyframeInterpolation, PROVEN_VALUE_BOUND, TimeCode};

    proptest! {
        /// MO1 R33 fallback (erratum MR1): H6 translation-equivariance as proptest
        /// plus segment-local reasoning — H6-N2 as Kani timed out at 900 s.
        ///
        /// Segment-local reasoning: [`super::shift_keys_keep_outside`] translates
        /// every key frame by `-delta` and preserves values, interpolations (and
        /// tangents via `..*`); [`super::value_at_keys`] depends only on relative
        /// positions — the window guard (`start.at <= at < end.at`), `span =
        /// end.at − start.at`, `offset = at − start.at`, and both clamp
        /// comparisons are invariant under a uniform `(keys, at)` translation by
        /// `-delta` — while `delta = end.value − start.value`, `linear`, `eased`,
        /// and the rounding see identical inputs on both sides. Hence
        /// `value_at(shift(k,d), t−d) == value_at(k,t)` for every `t`, with
        /// clamping translating uniformly. H4 proves the shift preserves keys and
        /// order; H1–H3 prove the evaluation properties; this proptest covers
        /// their composition over randomized curves, deltas, and frames.
        /// Randomized H6: shifted keys read the shifted frame identically.
        /// Values `±PROVEN_VALUE_BOUND`, all five kinds, `|delta| <= 32`,
        /// frames covering tails plus the keyed interval.
        #[test]
        fn proptest_translation_equivariance(
            base in 0..=8i64,
            gap_vals in prop::collection::vec(1..=8i64, 5),
            val_vals in prop::collection::vec(-PROVEN_VALUE_BOUND..=PROVEN_VALUE_BOUND, 6),
            tag_vals in prop::collection::vec(0..5u8, 6),
            key_count in 2..=6usize,
            delta in -32..=32i64,
            at in -8..=64i64,
        ) {
            let mut frames = Vec::with_capacity(key_count);
            let mut cursor = base;
            frames.push(cursor);
            for gap in gap_vals.iter().take(key_count - 1) {
                cursor += *gap;
                frames.push(cursor);
            }
            let keys: Vec<Keyframe> = (0..key_count)
                .map(|i| Keyframe {
                    at: TimeCode(frames[i]),
                    value: val_vals[i],
                    interpolation: match tag_vals[i] {
                        0 => KeyframeInterpolation::Hold,
                        1 => KeyframeInterpolation::Linear,
                        2 => KeyframeInterpolation::EaseIn,
                        3 => KeyframeInterpolation::EaseOut,
                        _ => KeyframeInterpolation::EaseInOut,
                    },
                    tangent_in: 0,
                    tangent_out: 0,
                })
                .collect();
            let orig = AutomationCurve { keyframes: keys };
            let shifted = super::rebase_clip_curve_keep_outside(&orig, TimeCode(delta));
            let expected = orig.value_at(TimeCode(at));
            let actual = shifted.value_at(TimeCode(at - delta));
            prop_assert!(expected.is_some(), "in-bounds reads Some at {at}");
            prop_assert_eq!(actual, expected);
        }
    }
}

/// MO1 R32 Kani proof harnesses: compiled only under `cargo kani`, which
/// passes `--cfg kani`. Never compiled into normal builds.
///
/// Admission rule (adopt-narrowly verdict, spike report
/// `target/review/kani/spike-report.md`; restated from `incident.rs`): a proof
/// is admitted only if it completes in < 5 min and < 6 GB. Symbolic strings
/// and map containers are out of scope. One `kani::assert` per harness:
/// a failing assert aborts the path, so each property lives in its own
/// harness rather than ahead of another check.
///
/// Shapes (all `i64`, values `±PROVEN_VALUE_BOUND` except H4's unbounded
/// values; unwind 8; bounds from the stage-0 probe `mo1-kani-probe.md`):
/// H1 between-keys bounded, H2 exact-key value, H3 clamp outside at N=2/3/4;
/// H4 shift round-trip identity at N=2/3/4. Twelve harnesses total.
/// Seam-H5 is dropped: it measured the AU4 drop+seam split, which stays
/// unproven (R32). H6 translation-equivariance is dropped as Kani: N=2 timed
/// out at 900 s (see erratum MR1) — the fallback is the proptest plus
/// segment-local reasoning in the unit tests below, not a silent drop (R33).
#[cfg(kani)]
mod mo1_proofs {
    use crate::{Keyframe, KeyframeInterpolation, TimeCode};

    const VMAX: i64 = super::PROVEN_VALUE_BOUND;

    fn kind_of(tag: u8) -> KeyframeInterpolation {
        match tag {
            0 => KeyframeInterpolation::Hold,
            1 => KeyframeInterpolation::Linear,
            2 => KeyframeInterpolation::EaseIn,
            3 => KeyframeInterpolation::EaseOut,
            4 => KeyframeInterpolation::EaseInOut,
            _ => unreachable!("harness assumes kind tag < 5"),
        }
    }

    fn sym_frames_2() -> [i64; 2] {
        let base: i64 = kani::any();
        let gap1: i64 = kani::any();
        kani::assume(0 <= base && base <= 8);
        kani::assume(1 <= gap1 && gap1 <= 8);
        [base, base + gap1]
    }

    fn sym_frames_3() -> [i64; 3] {
        let base: i64 = kani::any();
        let gap1: i64 = kani::any();
        let gap2: i64 = kani::any();
        kani::assume(0 <= base && base <= 8);
        kani::assume(1 <= gap1 && gap1 <= 8);
        kani::assume(1 <= gap2 && gap2 <= 8);
        [base, base + gap1, base + gap1 + gap2]
    }

    fn sym_frames_4() -> [i64; 4] {
        let base: i64 = kani::any();
        let gap1: i64 = kani::any();
        let gap2: i64 = kani::any();
        let gap3: i64 = kani::any();
        kani::assume(0 <= base && base <= 8);
        kani::assume(1 <= gap1 && gap1 <= 8);
        kani::assume(1 <= gap2 && gap2 <= 8);
        kani::assume(1 <= gap3 && gap3 <= 8);
        [
            base,
            base + gap1,
            base + gap1 + gap2,
            base + gap1 + gap2 + gap3,
        ]
    }

    fn sym_keys_2() -> [Keyframe; 2] {
        let ats = sym_frames_2();
        let values: [i64; 2] = kani::any();
        let tags: [u8; 2] = kani::any();
        kani::assume(-VMAX <= values[0] && values[0] <= VMAX);
        kani::assume(-VMAX <= values[1] && values[1] <= VMAX);
        kani::assume(tags[0] < 5 && tags[1] < 5);
        [
            Keyframe {
                at: TimeCode(ats[0]),
                value: values[0],
                interpolation: kind_of(tags[0]),
                tangent_in: 0,
                tangent_out: 0,
            },
            Keyframe {
                at: TimeCode(ats[1]),
                value: values[1],
                interpolation: kind_of(tags[1]),
                tangent_in: 0,
                tangent_out: 0,
            },
        ]
    }

    fn sym_keys_3() -> [Keyframe; 3] {
        let ats = sym_frames_3();
        let values: [i64; 3] = kani::any();
        let tags: [u8; 3] = kani::any();
        kani::assume(-VMAX <= values[0] && values[0] <= VMAX);
        kani::assume(-VMAX <= values[1] && values[1] <= VMAX);
        kani::assume(-VMAX <= values[2] && values[2] <= VMAX);
        kani::assume(tags[0] < 5 && tags[1] < 5 && tags[2] < 5);
        [
            Keyframe {
                at: TimeCode(ats[0]),
                value: values[0],
                interpolation: kind_of(tags[0]),
                tangent_in: 0,
                tangent_out: 0,
            },
            Keyframe {
                at: TimeCode(ats[1]),
                value: values[1],
                interpolation: kind_of(tags[1]),
                tangent_in: 0,
                tangent_out: 0,
            },
            Keyframe {
                at: TimeCode(ats[2]),
                value: values[2],
                interpolation: kind_of(tags[2]),
                tangent_in: 0,
                tangent_out: 0,
            },
        ]
    }

    fn sym_keys_4() -> [Keyframe; 4] {
        let ats = sym_frames_4();
        let values: [i64; 4] = kani::any();
        let tags: [u8; 4] = kani::any();
        kani::assume(-VMAX <= values[0] && values[0] <= VMAX);
        kani::assume(-VMAX <= values[1] && values[1] <= VMAX);
        kani::assume(-VMAX <= values[2] && values[2] <= VMAX);
        kani::assume(-VMAX <= values[3] && values[3] <= VMAX);
        kani::assume(tags[0] < 5 && tags[1] < 5 && tags[2] < 5 && tags[3] < 5);
        [
            Keyframe {
                at: TimeCode(ats[0]),
                value: values[0],
                interpolation: kind_of(tags[0]),
                tangent_in: 0,
                tangent_out: 0,
            },
            Keyframe {
                at: TimeCode(ats[1]),
                value: values[1],
                interpolation: kind_of(tags[1]),
                tangent_in: 0,
                tangent_out: 0,
            },
            Keyframe {
                at: TimeCode(ats[2]),
                value: values[2],
                interpolation: kind_of(tags[2]),
                tangent_in: 0,
                tangent_out: 0,
            },
            Keyframe {
                at: TimeCode(ats[3]),
                value: values[3],
                interpolation: kind_of(tags[3]),
                tangent_in: 0,
                tangent_out: 0,
            },
        ]
    }

    /// H4 inputs: values fully symbolic over `i64` (the shift never does
    /// value arithmetic, so H4 needs no value bound at all).
    fn sym_keys_shift_2() -> [Keyframe; 2] {
        let ats = sym_frames_2();
        let values: [i64; 2] = kani::any();
        let tags: [u8; 2] = kani::any();
        kani::assume(tags[0] < 5 && tags[1] < 5);
        [
            Keyframe {
                at: TimeCode(ats[0]),
                value: values[0],
                interpolation: kind_of(tags[0]),
                tangent_in: 0,
                tangent_out: 0,
            },
            Keyframe {
                at: TimeCode(ats[1]),
                value: values[1],
                interpolation: kind_of(tags[1]),
                tangent_in: 0,
                tangent_out: 0,
            },
        ]
    }

    fn sym_keys_shift_3() -> [Keyframe; 3] {
        let ats = sym_frames_3();
        let values: [i64; 3] = kani::any();
        let tags: [u8; 3] = kani::any();
        kani::assume(tags[0] < 5 && tags[1] < 5 && tags[2] < 5);
        [
            Keyframe {
                at: TimeCode(ats[0]),
                value: values[0],
                interpolation: kind_of(tags[0]),
                tangent_in: 0,
                tangent_out: 0,
            },
            Keyframe {
                at: TimeCode(ats[1]),
                value: values[1],
                interpolation: kind_of(tags[1]),
                tangent_in: 0,
                tangent_out: 0,
            },
            Keyframe {
                at: TimeCode(ats[2]),
                value: values[2],
                interpolation: kind_of(tags[2]),
                tangent_in: 0,
                tangent_out: 0,
            },
        ]
    }

    fn sym_keys_shift_4() -> [Keyframe; 4] {
        let ats = sym_frames_4();
        let values: [i64; 4] = kani::any();
        let tags: [u8; 4] = kani::any();
        kani::assume(tags[0] < 5 && tags[1] < 5 && tags[2] < 5 && tags[3] < 5);
        [
            Keyframe {
                at: TimeCode(ats[0]),
                value: values[0],
                interpolation: kind_of(tags[0]),
                tangent_in: 0,
                tangent_out: 0,
            },
            Keyframe {
                at: TimeCode(ats[1]),
                value: values[1],
                interpolation: kind_of(tags[1]),
                tangent_in: 0,
                tangent_out: 0,
            },
            Keyframe {
                at: TimeCode(ats[2]),
                value: values[2],
                interpolation: kind_of(tags[2]),
                tangent_in: 0,
                tangent_out: 0,
            },
            Keyframe {
                at: TimeCode(ats[3]),
                value: values[3],
                interpolation: kind_of(tags[3]),
                tangent_in: 0,
                tangent_out: 0,
            },
        ]
    }

    fn strictly_sorted(keys: &[Keyframe]) -> bool {
        let mut index = 0;
        let mut ok = true;
        while index + 1 < keys.len() {
            ok &= keys[index].at.0 < keys[index + 1].at.0;
            index += 1;
        }
        ok
    }

    /// H1 oracle: the `[min, max]` of the pair containing `at`. The kernel
    /// computes the *value*; this computes the *bounds* from key data, so a
    /// kernel-side easing bug still goes red.
    fn window_bounds(keys: &[Keyframe], at: i64) -> Option<(i64, i64)> {
        let mut index = 0;
        while index + 1 < keys.len() {
            if keys[index].at.0 <= at && at < keys[index + 1].at.0 {
                return Some((
                    keys[index].value.min(keys[index + 1].value),
                    keys[index].value.max(keys[index + 1].value),
                ));
            }
            index += 1;
        }
        None
    }

    /// H2 check: every key reads back its own value at its own frame.
    fn check_exact(keys: &[Keyframe]) -> bool {
        let mut ok = true;
        let mut index = 0;
        while index < keys.len() {
            ok &= super::value_at_keys(keys, keys[index].at.0) == Some(keys[index].value);
            index += 1;
        }
        ok
    }

    /// H3 check: at-or-outside either end clamps to the end key's value.
    fn check_clamp(keys: &[Keyframe], at: i64) -> bool {
        let first = keys[0];
        let last = keys[keys.len() - 1];
        match super::value_at_keys(keys, at) {
            Some(value) => {
                (!(at <= first.at.0) || value == first.value)
                    && (!(at >= last.at.0) || value == last.value)
            }
            None => false,
        }
    }

    /// H4 check: shift by `delta` then `-delta` is identity on all key
    /// fields, both shifted arrays stay strictly sorted, and the first
    /// shift moves every key by exactly `-delta` (review F4: the round
    /// trip alone passes with the shift sign flipped).
    fn check_shift_roundtrip<const N: usize>(keys: &[Keyframe; N], delta: i64) -> bool {
        let mut once = *keys;
        super::shift_keys_keep_outside(keys, delta, &mut once);
        let mut twice = once;
        super::shift_keys_keep_outside(&once, -delta, &mut twice);
        let mut ok = strictly_sorted(&once) && strictly_sorted(&twice);
        let mut index = 0;
        while index < N {
            ok &= twice[index] == keys[index];
            ok &= once[index].at.0 == keys[index].at.0 - delta;
            index += 1;
        }
        ok
    }

    // H1: between two keys, the value stays within [min, max] of the
    // containing pair, for all five interpolation kinds (all monotone).
    #[kani::proof]
    #[kani::unwind(8)]
    fn mo1_h1_n2() {
        let keys = sym_keys_2();
        let at: i64 = kani::any();
        kani::assume(keys[0].at.0 < at && at < keys[1].at.0);
        let value = super::value_at_keys(&keys, at);
        let ok = match (value, window_bounds(&keys, at)) {
            (Some(value), Some((lo, hi))) => lo <= value && value <= hi,
            _ => false,
        };
        kani::assert(ok, "H1/N2: between-keys value within [min,max]");
    }

    #[kani::proof]
    #[kani::unwind(8)]
    fn mo1_h1_n3() {
        let keys = sym_keys_3();
        let at: i64 = kani::any();
        kani::assume(keys[0].at.0 < at && at < keys[2].at.0);
        let value = super::value_at_keys(&keys, at);
        let ok = match (value, window_bounds(&keys, at)) {
            (Some(value), Some((lo, hi))) => lo <= value && value <= hi,
            _ => false,
        };
        kani::assert(ok, "H1/N3: between-keys value within [min,max]");
    }

    #[kani::proof]
    #[kani::unwind(8)]
    fn mo1_h1_n4() {
        let keys = sym_keys_4();
        let at: i64 = kani::any();
        kani::assume(keys[0].at.0 < at && at < keys[3].at.0);
        let value = super::value_at_keys(&keys, at);
        let ok = match (value, window_bounds(&keys, at)) {
            (Some(value), Some((lo, hi))) => lo <= value && value <= hi,
            _ => false,
        };
        kani::assert(ok, "H1/N4: between-keys value within [min,max]");
    }

    // H2: evaluation at a key's exact frame equals that key's value.
    #[kani::proof]
    #[kani::unwind(8)]
    fn mo1_h2_n2() {
        let keys = sym_keys_2();
        kani::assert(
            check_exact(&keys),
            "H2/N2: exact key frames read back key values",
        );
    }

    #[kani::proof]
    #[kani::unwind(8)]
    fn mo1_h2_n3() {
        let keys = sym_keys_3();
        kani::assert(
            check_exact(&keys),
            "H2/N3: exact key frames read back key values",
        );
    }

    #[kani::proof]
    #[kani::unwind(8)]
    fn mo1_h2_n4() {
        let keys = sym_keys_4();
        kani::assert(
            check_exact(&keys),
            "H2/N4: exact key frames read back key values",
        );
    }

    // H3: evaluation clamps outside the first/last key.
    #[kani::proof]
    #[kani::unwind(8)]
    fn mo1_h3_n2() {
        let keys = sym_keys_2();
        let at: i64 = kani::any();
        kani::assume(-8 <= at && at <= 40);
        kani::assert(
            check_clamp(&keys, at),
            "H3/N2: evaluation clamps outside the keyed interval",
        );
    }

    #[kani::proof]
    #[kani::unwind(8)]
    fn mo1_h3_n3() {
        let keys = sym_keys_3();
        let at: i64 = kani::any();
        kani::assume(-8 <= at && at <= 40);
        kani::assert(
            check_clamp(&keys, at),
            "H3/N3: evaluation clamps outside the keyed interval",
        );
    }

    #[kani::proof]
    #[kani::unwind(8)]
    fn mo1_h3_n4() {
        let keys = sym_keys_4();
        let at: i64 = kani::any();
        kani::assume(-8 <= at && at <= 40);
        kani::assert(
            check_clamp(&keys, at),
            "H3/N4: evaluation clamps outside the keyed interval",
        );
    }

    // H4: keep-outside trim-shift then un-trim-shift is identity on keys.
    #[kani::proof]
    #[kani::unwind(8)]
    fn mo1_h4_n2() {
        let keys = sym_keys_shift_2();
        let delta: i64 = kani::any();
        kani::assume(-32 <= delta && delta <= 32);
        kani::assert(
            check_shift_roundtrip(&keys, delta),
            "H4/N2: shift then un-shift is identity on keys",
        );
    }

    #[kani::proof]
    #[kani::unwind(8)]
    fn mo1_h4_n3() {
        let keys = sym_keys_shift_3();
        let delta: i64 = kani::any();
        kani::assume(-32 <= delta && delta <= 32);
        kani::assert(
            check_shift_roundtrip(&keys, delta),
            "H4/N3: shift then un-shift is identity on keys",
        );
    }

    #[kani::proof]
    #[kani::unwind(8)]
    fn mo1_h4_n4() {
        let keys = sym_keys_shift_4();
        let delta: i64 = kani::any();
        kani::assume(-32 <= delta && delta <= 32);
        kani::assert(
            check_shift_roundtrip(&keys, delta),
            "H4/N4: shift then un-shift is identity on keys",
        );
    }
}
