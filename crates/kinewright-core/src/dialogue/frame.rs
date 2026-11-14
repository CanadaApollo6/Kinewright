// Derived from Scarlet packages/mcp/src/dialogue-join.ts at 2cbab542; see LICENSES/scarlet-MIT.txt.
use super::{Dialogue, Hop, Refusal, RefusalReason, ResolvedEdge};
use crate::{Rational, TimeCode};
use serde::Serialize;

/// The exact fractional source microsecond instant is numerator / denominator.
#[derive(Debug, PartialEq, Eq, Serialize)]
pub struct FramedEdge {
    pub frame: TimeCode,
    pub instant_numerator: u128,
    pub instant_denominator: u32,
    /// Signed rounding shift, in microseconds over `instant_denominator`.
    pub rounding_shift_numerator: i128,
    /// Reading at the returned frame, not the original minimum hop. None at EOF.
    pub measured_hop: Option<Hop>,
}

impl Dialogue {
    /// The supplied frame grid is anchored to source time zero, usually the
    /// asset's frame rate. Clip/project offset mapping belongs to the caller.
    ///
    /// Choose the nearest frame inside the measured interval, rounding inward
    /// when ordinary nearest rounding leaves it. Check the exact fractional
    /// instant again, never an integer-microsecond approximation of that frame.
    ///
    /// # Errors
    /// Refuses invalid rates, overflow, no frame in the interval, or a failed
    /// post-rounding acoustic guard (including evidence from another profile).
    pub fn frame_edge(&self, edge: ResolvedEdge, fps: Rational) -> Result<FramedEdge, Refusal> {
        if !fps.is_valid() {
            return Err(self.refuse(RefusalReason::FrameRate, edge.at_micros));
        }
        let n = u128::from(fps.numerator());
        let scale = 1_000_000 * u128::from(fps.denominator());
        let support = edge.support;
        let first = (u128::from(support.start) * n).div_ceil(scale);
        let last = if support.inclusive_end {
            u128::from(support.end) * n / scale
        } else {
            (u128::from(support.end) * n)
                .div_ceil(scale)
                .saturating_sub(1)
        };
        if first > last {
            return Err(self.refuse(RefusalReason::NoFrameInRun, edge.at_micros));
        }
        let nearest = (u128::from(edge.at_micros) * n + scale / 2) / scale;
        let selected = nearest.clamp(first, last);
        let frame = i64::try_from(selected)
            .map_err(|_| self.refuse(RefusalReason::TimeOverflow, edge.at_micros))?;
        let instant_numerator = selected * scale;
        self.guard_ratio(instant_numerator, n, edge.side)?;
        let rounding_shift_numerator = i128::try_from(instant_numerator)
            .map_err(|_| self.refuse(RefusalReason::TimeOverflow, edge.at_micros))?
            - i128::from(edge.at_micros) * i128::from(fps.numerator());
        let measured_hop = self
            .profile
            .index_at(instant_numerator, n)
            .and_then(|index| self.profile.hop(index));
        Ok(FramedEdge {
            frame: TimeCode(frame),
            instant_numerator,
            instant_denominator: fps.numerator(),
            rounding_shift_numerator,
            measured_hop,
        })
    }
}
