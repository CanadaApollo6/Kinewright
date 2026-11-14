// Derived from Scarlet packages/mcp/src/dialogue-join.ts at 2cbab542; see LICENSES/scarlet-MIT.txt.
use super::{AudibleSide, Dialogue, Hop, Refusal, RefusalReason, ResolvedEdge};
use crate::{AudibleClockError, ExactMicros, FrameRounding, Rational, TimeCode};
use serde::Serialize;

/// The exact fractional source microsecond instant is numerator / denominator.
#[derive(Debug, PartialEq, Eq, Serialize)]
pub struct FramedEdge {
    pub frame: TimeCode,
    pub instant_numerator: u128,
    pub instant_denominator: u64,
    /// Signed rounding shift, in microseconds over `instant_denominator`.
    pub rounding_shift_numerator: i128,
    /// Reading at the returned frame, not the original minimum hop. None at EOF.
    pub measured_hop: Option<Hop>,
}

/// A frame grid anchored to a source in-point, independent of absolute zero.
#[derive(Debug, Clone, Copy)]
pub struct FrameGrid {
    origin: ExactMicros,
    fps: Rational,
}
impl FrameGrid {
    /// # Errors
    /// Refuses an invalid rate.
    pub fn new(origin: ExactMicros, fps: Rational) -> Result<Self, AudibleClockError> {
        if !fps.is_valid() {
            return Err(AudibleClockError);
        }
        Ok(Self { origin, fps })
    }
    fn position(self, at: ExactMicros) -> Result<(u128, u128), AudibleClockError> {
        let delta = at.checked_sub(self.origin)?;
        let n = delta
            .numerator()
            .checked_mul(u128::from(self.fps.numerator()))
            .ok_or(AudibleClockError)?;
        let d = u128::from(delta.denominator()) * 1_000_000 * u128::from(self.fps.denominator());
        Ok((n, d))
    }
    /// # Errors
    /// Refuses an instant before this grid or unrepresentable frame arithmetic.
    pub fn frame_at(
        self,
        at: ExactMicros,
        rounding: FrameRounding,
    ) -> Result<TimeCode, AudibleClockError> {
        let (n, d) = self.position(at)?;
        let frame = match rounding {
            FrameRounding::Floor => n / d,
            FrameRounding::Ceil => n.div_ceil(d),
            FrameRounding::Nearest => n / d + u128::from(n % d >= d.div_ceil(2)),
        };
        Ok(TimeCode(
            i64::try_from(frame).map_err(|_| AudibleClockError)?,
        ))
    }
    /// # Errors
    /// Refuses negative frames or time overflow.
    pub fn instant(self, frame: TimeCode) -> Result<ExactMicros, AudibleClockError> {
        self.origin
            .checked_add(ExactMicros::frames(frame, self.fps)?)
    }
}
impl Dialogue {
    /// Nearest supported frame on the asset grid, anchored to source zero.
    /// # Errors
    /// Refuses a missing frame or a failed exact post-rounding guard.
    pub fn frame_edge(&self, edge: ResolvedEdge, fps: Rational) -> Result<FramedEdge, Refusal> {
        let grid = FrameGrid::new(ExactMicros::ZERO, fps)
            .map_err(|_| self.refuse(RefusalReason::FrameRate, edge.at_micros))?;
        self.frame_edge_on_grid(edge, grid)
    }
    /// Nearest supported frame on a grid anchored to the clip's source in-point.
    /// Both grids share the same exact projection and acoustic recheck.
    /// # Errors
    /// Refuses a missing frame, overflow, or a failed exact acoustic guard.
    pub fn frame_edge_on_grid(
        &self,
        edge: ResolvedEdge,
        grid: FrameGrid,
    ) -> Result<FramedEdge, Refusal> {
        let failure = || self.refuse(RefusalReason::TimeOverflow, edge.at_micros);
        let support = edge.support;
        let first = if ExactMicros::integer(support.start) < grid.origin {
            0
        } else {
            grid.frame_at(ExactMicros::integer(support.start), FrameRounding::Ceil)
                .map_err(|_| failure())?
                .0
        };
        let end = ExactMicros::integer(support.end);
        if end < grid.origin {
            return Err(self.refuse(RefusalReason::NoFrameInRun, edge.at_micros));
        }
        let last = if support.inclusive_end {
            grid.frame_at(end, FrameRounding::Floor)
                .map_err(|_| failure())?
                .0
        } else {
            grid.frame_at(end, FrameRounding::Ceil)
                .map_err(|_| failure())?
                .0
                - 1
        };
        if first > last {
            return Err(self.refuse(RefusalReason::NoFrameInRun, edge.at_micros));
        }
        let at = ExactMicros::integer(edge.at_micros).max(grid.origin);
        let (n, d) = grid.position(at).map_err(|_| failure())?;
        let rounding = if edge.side == AudibleSide::In && n % d == d / 2 && d % 2 == 0 {
            FrameRounding::Floor
        } else {
            FrameRounding::Nearest
        };
        let frame = TimeCode(
            grid.frame_at(at, rounding)
                .map_err(|_| failure())?
                .0
                .clamp(first, last),
        );
        let instant = grid.instant(frame).map_err(|_| failure())?;
        self.guard_exact(instant, edge.side)?;
        // Preserve the asset-grid DTO denominator while also accommodating
        // fractional source origins on the project grid.
        let den = u64::from(grid.fps.numerator());
        let common = crate::audible::gcd(den, instant.denominator());
        let denominator = den
            .checked_mul(instant.denominator() / common)
            .ok_or_else(failure)?;
        let numerator = instant
            .numerator()
            .checked_mul(u128::from(denominator / instant.denominator()))
            .ok_or_else(failure)?;
        let unrounded = i128::from(edge.at_micros)
            .checked_mul(i128::from(denominator))
            .ok_or_else(failure)?;
        let shift = i128::try_from(numerator)
            .map_err(|_| failure())?
            .checked_sub(unrounded)
            .ok_or_else(failure)?;
        Ok(FramedEdge {
            frame,
            instant_numerator: numerator,
            instant_denominator: denominator,
            rounding_shift_numerator: shift,
            measured_hop: self
                .profile
                .index_at(instant.numerator(), u128::from(instant.denominator()))
                .and_then(|i| self.profile.hop(i)),
        })
    }
}
