use std::ops::Range;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// A frame count in the time base named by the surrounding value.
///
/// Project positions use the project's frame rate. `Clip::source_range` and
/// `MediaAsset::duration` use that asset's frame rate.
#[derive(
    Debug,
    Default,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Serialize,
    Deserialize,
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
    /// Construct a reduced, positive rational value.
    ///
    /// # Errors
    ///
    /// Returns [`TimeMappingError::InvalidRate`] if either component is zero.
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

/// Scale a source frame rate by an integer playback speed percentage.
///
/// A clip at `speed_percent` consumes source frames as if the source ran at
/// `fps * speed_percent / 100` — 50 percent halves the effective rate (slow
/// motion doubles the project duration), 200 percent doubles it. The result
/// stays an exact reduced rational, so every source-to-project mapping built
/// on it remains integer-exact.
///
/// # Errors
///
/// Returns [`TimeMappingError::InvalidRate`] when the speed is zero or the
/// scaled numerator/denominator overflow `u32`.
pub fn speed_scaled_fps(fps: Rational, speed_percent: u32) -> Result<Rational, TimeMappingError> {
    if speed_percent == 100 {
        return Ok(fps);
    }
    let numerator = fps
        .numerator()
        .checked_mul(speed_percent)
        .ok_or(TimeMappingError::Overflow)?;
    let denominator = fps
        .denominator()
        .checked_mul(100)
        .ok_or(TimeMappingError::Overflow)?;
    Rational::new(numerator, denominator)
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
    #[error(
        "frame rates must have positive numerator and denominator, got {numerator}/{denominator}"
    )]
    InvalidRate { numerator: u32, denominator: u32 },
    #[error("negative frame counts cannot be mapped: {0}")]
    NegativeFrames(TimeCode),
    #[error("frame-rate conversion overflowed")]
    Overflow,
    #[error("source range must be non-empty and non-negative: {start}..{end}")]
    InvalidRange { start: i64, end: i64 },
    /// AU5 §5.3: no source end maps to exactly the requested project duration.
    ///
    /// `map_frames` rounds to nearest, so a project running faster than the
    /// source cannot express every span: a 60 fps project reading a 30 fps
    /// audio-only asset can only express even-frame durations. The caller is
    /// told, never silently given the wrong length.
    #[error(
        "no source range from frame {source_start} maps to exactly {project_duration} project frames"
    )]
    InexactDuration {
        source_start: i64,
        project_duration: i64,
    },
    /// AU5 §0 R96: exact source ranges exist, but no phase at these two frame
    /// rates carries enough sample frames to fill the gap.
    ///
    /// **Not** a statement about the asset's length — that is
    /// [`TimeMappingError::SourceTooShortToCover`], which is checked first and
    /// separately. This one is a limit of the one-for-one sample mapping: at
    /// 30 → 59.94 no even project span is coverable at any phase or any
    /// length, because one source frame supplies 1 600 sample frames against a
    /// demand of 1 602 while two source frames map to two project frames too
    /// many. Telling an editor to record more room tone would be wrong.
    #[error(
        "no source range at these frame rates covers {project_duration} project frames within {source_duration} source frames"
    )]
    NoCoveringSourceRange {
        project_duration: i64,
        source_duration: i64,
    },
    /// AU5 §0 R96: the asset is provably too short to fill the gap, whatever
    /// the phase.
    ///
    /// Supply is at most `length × sample_rate × source_fps.denominator /
    /// source_fps.numerator`, so no range shorter than
    /// `minimum_source_frames` can carry the gap's sample frames at any phase.
    /// This is checked before the phase sweep runs, so the message is a proof
    /// rather than a summary of a failed search, and it is the one refusal
    /// whose answer really is "record more room tone".
    #[error(
        "filling {project_duration} project frames needs at least {minimum_source_frames} source frames, but the asset carries only {source_duration}"
    )]
    SourceTooShortToCover {
        project_duration: i64,
        source_duration: i64,
        minimum_source_frames: i64,
    },
}

/// Map a frame boundary between time bases using integer, round-to-nearest math.
///
/// # Errors
///
/// Returns an error for invalid rates, negative frames, or arithmetic overflow.
pub fn map_frames(
    frames: TimeCode,
    source_fps: Rational,
    project_fps: Rational,
) -> Result<TimeCode, TimeMappingError> {
    map_frames_with_rounding(frames, source_fps, project_fps, FrameRounding::Nearest)
}

/// Map a frame boundary between time bases without passing through seconds or floats.
///
/// # Errors
///
/// Returns an error for invalid rates, negative frames, or arithmetic overflow.
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
        FrameRounding::Nearest => {
            numerator
                .checked_add(denominator / 2)
                .ok_or(TimeMappingError::Overflow)?
                / denominator
        }
        FrameRounding::Ceil => {
            numerator
                .checked_add(denominator - 1)
                .ok_or(TimeMappingError::Overflow)?
                / denominator
        }
    };

    i64::try_from(mapped)
        .map(TimeCode)
        .map_err(|_| TimeMappingError::Overflow)
}

/// Map an asset range by mapping both absolute boundaries, then subtracting.
///
/// Mapping absolute boundaries is important: adjacent ranges share the exact
/// same mapped boundary, so mixed-rate splits cannot create cumulative drift.
///
/// # Errors
///
/// Returns an error for an invalid range or any frame-mapping failure.
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

/// AU5 §5.3: the **smallest** source end whose mapped project span is exactly
/// `project_duration`.
///
/// This is the inverse of [`map_source_range_to_project`], and is defined by
/// it: every candidate is confirmed by running the forward mapping, so the two
/// cannot disagree about what a source range is worth.
///
/// **Why the two-frame search window finds an exact end whenever one exists.**
/// Write `r = source_fps / project_fps` for the source frames a project frame
/// is worth, and `a = map_frames(source_start)`. Write `c` for this module's
/// rounding threshold: [`FrameRounding::Nearest`] is
/// `(numerator + denominator / 2) / denominator` with an **integer**
/// `denominator / 2`, so on an odd `den` — the product of the source
/// numerator and the project denominator, 25 for 25 → 24, so odd is
/// ordinary — `c = ⌊den/2⌋ / den = 0.5 − 1/(2·den)` rather than `0.5`. The ends
/// that map exactly are then the integers in
/// `[r·(a + duration − c), r·(a + duration + 1 − c))`, a half-open interval of
/// width exactly `r` that always contains the real point
/// `x = source_start + r·duration`. That containment is an **identity, not an
/// approximation**: the same `c` is what rounded `source_start` to `a`, so
/// `r·(a − c) ≤ source_start < r·(a + 1 − c)` is the definition of `a`, and
/// adding `r·duration` to all three carries it. A nearest integer to `x` is
/// therefore either in the interval or one step outside it, and
/// `estimate = map_frames(duration)` is within one frame of `r·duration` (it
/// needs only `c ∈ [0, 0.5]`, true for every denominator), which puts
/// `source_start + estimate` beside `x`. The window
/// `estimate − 1 ..= estimate + 1` therefore covers both integers adjacent to
/// `x`, and when `r ≥ 1` at least one of them lies in an interval that wide.
/// When `r < 1` the interval can hold no integer at all, and that is the
/// refusal below, not a miss.
///
/// **Why the window is not enough for minimality.** That interval is `r`
/// frames wide, so it holds `⌊r⌋` or `⌈r⌉` integers: whenever `r > 1` and the
/// interval straddles two of them the exact ends form a *run*, and the window
/// can land anywhere in it. Two different thresholds, not one — the run is
/// multi-member from `r > 1` up (at 30 → 24, `r = 1.25`, a two-frame span is
/// satisfied by both `0..2` and `0..3`), while the *window* only starts landing
/// on a non-minimal member around `r ≳ 1.6` (at 60 → 24 from `source_start =
/// 1`, a one-frame span is satisfied by both `1..2` and `1..3`, and the
/// estimate points at `1..4`'s neighbourhood). The smallest is recovered by
/// bisecting `source_start + 1 ..= end` on the forward mapping, which is
/// monotone non-decreasing in `end`, so the ends mapping to at least
/// `project_duration` are upward-closed and the first of them is the smallest
/// exact one. Bisection rather than a walk down: the run is `⌈r⌉` long and
/// nothing in this module bounds a frame rate, so a linear walk would be
/// `O(source_fps / project_fps)` on a hand-built document.
///
/// **Smallest is a decision, not a convenience (AU5 §0 R93, §0 R101).** The
/// mixer maps source samples to project samples one for one, so the smaller
/// end of a multi-member run leaves up to one asset frame of silence at the
/// fill's tail that the larger end would have removed. Smallest is kept
/// because a "largest" rule can push the end past `asset.duration`, which the
/// planner's fill ladder has no arm for, and because the residual is reported
/// per gap rather than hidden. R93 carries the arithmetic and the re-opening
/// conditions.
///
/// [`map_frames`] rounds to nearest (`FrameRounding::Nearest`), so not every
/// project duration is representable: a 60 fps project reading a 30 fps
/// audio-only asset can only express even-frame spans. The caller is told
/// through [`TimeMappingError::InexactDuration`], never silently given the
/// wrong length.
///
/// A `project_duration` of **zero** is refused uniformly, through the
/// empty-range error [`map_source_range_to_project`] itself answers for
/// `source_start..source_start`. A project slower than its source *can* map a
/// non-empty source range to zero project frames, so answering zero would make
/// the result rate-dependent for an input no fill can use.
///
/// # Errors
///
/// Returns an error for invalid rates, a negative `source_start` or
/// `project_duration`, a zero `project_duration`, arithmetic overflow, or a
/// duration with no exact source range at these two rates.
pub fn map_project_duration_to_source(
    source_start: TimeCode,
    project_duration: TimeCode,
    source_fps: Rational,
    project_fps: Rational,
) -> Result<TimeCode, TimeMappingError> {
    validate_rate(source_fps)?;
    validate_rate(project_fps)?;
    if source_start.0 < 0 {
        return Err(TimeMappingError::NegativeFrames(source_start));
    }
    if project_duration.0 < 0 {
        return Err(TimeMappingError::NegativeFrames(project_duration));
    }
    if project_duration.0 == 0 {
        return Err(TimeMappingError::InvalidRange {
            start: source_start.0,
            end: source_start.0,
        });
    }

    let exact = |end: TimeCode| {
        map_source_range_to_project(source_start..end, source_fps, project_fps)
            .is_ok_and(|mapped| mapped == project_duration)
    };

    let estimate = map_frames(project_duration, project_fps, source_fps)?;
    let mut found = None;
    for offset in -1..=1_i64 {
        let Some(end) = estimate
            .0
            .checked_add(offset)
            .and_then(|frames| frames.checked_add(source_start.0))
            .map(TimeCode)
        else {
            continue;
        };
        if end > source_start && exact(end) {
            found = Some(end);
            break;
        }
    }
    let Some(end) = found else {
        return Err(TimeMappingError::InexactDuration {
            source_start: source_start.0,
            project_duration: project_duration.0,
        });
    };

    let mut low = source_start.0.saturating_add(1);
    let mut high = end.0;
    while low < high {
        let middle = low + (high - low) / 2;
        if map_source_range_to_project(source_start..TimeCode(middle), source_fps, project_fps)
            .is_ok_and(|mapped| mapped >= project_duration)
        {
            high = middle;
        } else {
            low = middle + 1;
        }
    }
    debug_assert!(exact(TimeCode(low)), "bisection left the exact run");
    Ok(TimeCode(low))
}

/// The widest phase sweep [`covering_source_range_for_project_duration`] will
/// pay for.
///
/// The rule is one whole `period` (see that function); this is the cap that
/// keeps it bounded, because nothing in this module bounds a frame rate and
/// the period reaches ~1.8e19 on a hand-built `u32` pair. **4 096 admits every
/// workspace rate pair's whole period with headroom**: the widest is 2 500, at
/// **59.94 → 24**. Among the nine rates the workspace uses — 30, 25, 24,
/// 23.976, 29.97, 59.94, 60, 50, 48 — the deepest phase any gap of 1..=200
/// frames actually needs is 499, on the NTSC pulldown pairs. The sweep costs
/// one bisection per phase and only pays for the deep ones on a gap that
/// would otherwise be refused.
const MAX_PHASE_CANDIDATES: i64 = 4_096;

/// AU5 §5.3 / §0 R96: a source range that maps to exactly `project_duration`
/// project frames **and carries enough sample frames to fill it**.
///
/// [`map_project_duration_to_source`] answers a range that is exact in project
/// *frames*. That is not the same as a range that fills the gap, because the
/// audio mixer maps source samples to project samples **one for one**: it
/// opens the decoder over the clip's source range, plays the mapped project
/// span and stops, so a fill whose source range carries fewer sample frames
/// than the gap needs runs out early and leaves silence, while one carrying
/// *more* is cleanly truncated — nothing ties the source length to the project
/// span, and a longer range is neither an overlap nor a validation failure.
///
/// **Which exact ends exist depends on the phase of `source_start`.** At
/// 30 → 25 a seven-frame gap needs 13 440 sample frames and admits `0..8` and
/// `1..9` (12 800 each, short), `1..10` and `2..11` (14 400, covering) and
/// `3..12`. A caller hard-coding `source_start = 0` therefore ships the one
/// range that cannot cover. This helper therefore sweeps the phase over **one whole
/// period** of the pattern (derived at the loop below): for each
/// `source_start` it takes that phase's smallest exact end and walks the exact
/// run upward, returning the first range that supplies at least the gap's
/// sample frames and ends at or before `source_duration`. Candidates are tried
/// in increasing `source_start`, then increasing `end`, so the answer consumes
/// as little of the asset as covering allows.
///
/// **The sample arithmetic is media's, re-spelled.** `kinewright-media`'s
/// `clock::frame_to_samples` is
/// `⌊frame × sample_rate × fps.denominator / fps.numerator⌋` on `u128`, zero
/// for a non-positive frame; core cannot call it — media depends on core, not
/// the other way round — so it is written out again here and this comment is
/// the pin. The **supply** side uses that rule exactly, at `source_fps`. The
/// **demand** side rounds the other way, `⌈project_duration × sample_rate ×
/// project_fps.denominator / project_fps.numerator⌉`, because the mixer's real
/// demand is `⌊end × k⌋ − ⌊start × k⌋` for the gap's absolute position, which
/// is `⌊D·k⌋` or `⌈D·k⌉` depending where the gap sits; the ceiling is the
/// position-independent bound, and the two agree for every rate the workspace
/// uses at 48 kHz (`k` is 1 600, 1 920, 2 000 and 2 002 at 30, 25, 24 and
/// 23.976 fps — all integers).
///
/// # Errors
///
/// Returns [`TimeMappingError::InvalidRate`] for an invalid frame rate or a
/// zero `sample_rate`, and [`TimeMappingError::InvalidRange`] for a
/// non-positive `project_duration` or `source_duration`. The three refusals a
/// caller branches on, in the order they are decided:
///
/// - [`TimeMappingError::SourceTooShortToCover`] — the asset is provably too
///   short, whatever the phase. Decided before the sweep, from the supply
///   bound alone. The one refusal whose answer is "record more room tone".
/// - [`TimeMappingError::InexactDuration`] — no phase expresses the duration
///   at these two frame rates, so no *exact* fill exists at all.
/// - [`TimeMappingError::NoCoveringSourceRange`] — exact ranges exist and the
///   asset is long enough, but none carries the gap's sample frames at any
///   phase or any length. A limit of the one-for-one sample mapping.
pub fn covering_source_range_for_project_duration(
    project_duration: TimeCode,
    source_fps: Rational,
    project_fps: Rational,
    source_duration: TimeCode,
    sample_rate: u32,
) -> Result<Range<TimeCode>, TimeMappingError> {
    validate_rate(source_fps)?;
    validate_rate(project_fps)?;
    Rational::new(sample_rate, 1)?;
    if project_duration.0 <= 0 {
        return Err(TimeMappingError::InvalidRange {
            start: 0,
            end: project_duration.0,
        });
    }
    if source_duration.0 <= 0 {
        return Err(TimeMappingError::InvalidRange {
            start: 0,
            end: source_duration.0,
        });
    }

    let required = required_sample_frames(project_duration, sample_rate, project_fps);

    let minimum_source_frames = i64::try_from(
        required
            .saturating_mul(u128::from(source_fps.numerator()))
            .div_ceil(u128::from(sample_rate).saturating_mul(u128::from(source_fps.denominator()))),
    )
    .unwrap_or(i64::MAX);
    if source_duration.0 < minimum_source_frames {
        return Err(TimeMappingError::SourceTooShortToCover {
            project_duration: project_duration.0,
            source_duration: source_duration.0,
            minimum_source_frames,
        });
    }

    let ratio_numerator =
        i128::from(source_fps.numerator()) * i128::from(project_fps.denominator());
    let ratio_denominator =
        i128::from(source_fps.denominator()) * i128::from(project_fps.numerator());
    let map_period = ratio_numerator / gcd_i128(ratio_numerator, ratio_denominator);
    let sample_period = i128::from(source_fps.numerator())
        / gcd_i128(
            i128::from(sample_rate) * i128::from(source_fps.denominator()),
            i128::from(source_fps.numerator()),
        );
    let period = (map_period / gcd_i128(map_period, sample_period)).saturating_mul(sample_period);
    let phase_candidates = i64::try_from(period)
        .unwrap_or(i64::MAX)
        .clamp(1, MAX_PHASE_CANDIDATES);

    let mut any_exact = false;

    for start in 0..phase_candidates {
        let start = TimeCode(start);
        let Ok(smallest) =
            map_project_duration_to_source(start, project_duration, source_fps, project_fps)
        else {
            continue;
        };
        any_exact = true;
        let mut end = smallest;
        while end <= source_duration {
            if frame_sample_position(end, sample_rate, source_fps)
                .saturating_sub(frame_sample_position(start, sample_rate, source_fps))
                >= required
            {
                return Ok(start..end);
            }
            let next = TimeCode(end.0.saturating_add(1));
            if next <= end
                || map_source_range_to_project(start..next, source_fps, project_fps)
                    != Ok(project_duration)
            {
                break;
            }
            end = next;
        }
    }

    if any_exact {
        Err(TimeMappingError::NoCoveringSourceRange {
            project_duration: project_duration.0,
            source_duration: source_duration.0,
        })
    } else {
        Err(TimeMappingError::InexactDuration {
            source_start: 0,
            project_duration: project_duration.0,
        })
    }
}

/// [`gcd`] on the width the ratio arithmetic uses.
const fn gcd_i128(mut lhs: i128, mut rhs: i128) -> i128 {
    while rhs != 0 {
        let remainder = lhs % rhs;
        lhs = rhs;
        rhs = remainder;
    }
    lhs
}

/// The widest step-down [`longest_coverable_project_tile`] will pay for.
///
/// Coverability is **not monotone** in `want`, so the search is a first-hit
/// downward scan rather than a bisection, and the scan needs a bound. 2 048
/// gives **4.09x** headroom over the worst step there is at a workspace rate
/// pair, found by sweeping all 81 ordered pairs of the nine rates over
/// `source_duration` 30..=1 800: **501**, at 23.976 → 24 with a 999-frame asset
/// (`whole` = 1 000, first coverable `want` = 499). The 30 → 29.97 case is the
/// memorable one rather than the worst — 499, for a 1 500-frame asset whose
/// only covering tile is the map period's `0..1001`.
const MAX_TILE_SEARCH_STEPS: i64 = 2_048;

/// AU5 §0 R97: the longest project span this asset can tile, and the source
/// range that tiles it.
///
/// A room-tone fill repeats **one** tile, so a caller needs the largest `want`
/// its asset can actually cover, not merely the largest it maps to. The two
/// differ, and a tiler that walks down from the asset's own mapped length by a
/// small literal misses the difference badly: at 30 → 29.97 the covering tile
/// is the map period's `0..1001` — 1 001 source frames worth 1 000 project
/// frames — so an asset of 1 006..=1 500 frames has to step down by 5..=499 to
/// reach it. Against a four-frame allowance, **496 of the 1 300
/// store-producible asset lengths** fail at 30 → 29.97 and **744** at
/// 30 → 59.94, and because a tiler that finds no tile skips *every* gap on the
/// track, an ordinary 33–50 s NTSC capture fills nothing at all. That is the
/// same user-visible failure R96's phase window was written to close, and it
/// is why this search belongs beside it rather than being spelled once per
/// caller.
///
/// Returns the largest `want` **within [`MAX_TILE_SEARCH_STEPS`] steps** of
/// `min(max_project_frames, whole)` — where `whole` is the project span the
/// asset maps to — for which [`covering_source_range_for_project_duration`]
/// answers, together with that range. The qualifier is not decorative: that
/// ceiling passes 2 048 on real input, since a 1 800-frame (60 s, the capture
/// cap) tone at 30 → 59.94 maps to 3 596 project frames. It is harmless there,
/// because coverable wants at that pair are two apart, but a caller reading
/// this as an unconditional maximum would be wrong. `max_project_frames` is the gap the caller wants to fill, so the
/// tile never overshoots it.
///
/// **The scan is a first-hit downward walk, one frame at a time, because
/// coverability is not monotone in `want`.** A `want` that fails says nothing
/// about `want − 1`: at 30 → 59.94 the odd spans cover and the even ones cannot,
/// at any phase or length. So a bisection would be unsound, and the walk is
/// bounded by [`MAX_TILE_SEARCH_STEPS`] instead. Cost is that bound times one
/// phase sweep; it is paid only while nothing fits.
///
/// # Errors
///
/// Returns the rate and range errors
/// [`covering_source_range_for_project_duration`] returns, and when no `want`
/// in the searched span is coverable, **that helper's own last refusal** —
/// [`TimeMappingError::SourceTooShortToCover`],
/// [`TimeMappingError::InexactDuration`] or
/// [`TimeMappingError::NoCoveringSourceRange`] — so a caller's message names
/// the real reason rather than inventing one.
pub fn longest_coverable_project_tile(
    source_duration: TimeCode,
    source_fps: Rational,
    project_fps: Rational,
    sample_rate: u32,
    max_project_frames: TimeCode,
) -> Result<(TimeCode, Range<TimeCode>), TimeMappingError> {
    validate_rate(source_fps)?;
    validate_rate(project_fps)?;
    Rational::new(sample_rate, 1)?;
    if source_duration.0 <= 0 {
        return Err(TimeMappingError::InvalidRange {
            start: 0,
            end: source_duration.0,
        });
    }
    if max_project_frames.0 <= 0 {
        return Err(TimeMappingError::InvalidRange {
            start: 0,
            end: max_project_frames.0,
        });
    }

    let whole =
        map_source_range_to_project(TimeCode::ZERO..source_duration, source_fps, project_fps)?;
    let ceiling = whole.min(max_project_frames);

    let mut refusal = None;
    let mut want = ceiling.0;
    for _ in 0..MAX_TILE_SEARCH_STEPS {
        if want < 1 {
            break;
        }
        match covering_source_range_for_project_duration(
            TimeCode(want),
            source_fps,
            project_fps,
            source_duration,
            sample_rate,
        ) {
            Ok(range) => return Ok((TimeCode(want), range)),
            Err(error) => refusal = Some(error),
        }
        want -= 1;
    }

    Err(refusal.unwrap_or_else(|| {
        covering_source_range_for_project_duration(
            TimeCode(1),
            source_fps,
            project_fps,
            source_duration,
            sample_rate,
        )
        .err()
        .unwrap_or(TimeMappingError::NoCoveringSourceRange {
            project_duration: 1,
            source_duration: source_duration.0,
        })
    }))
}

/// `kinewright-media`'s `clock::frame_to_samples`, re-spelled — see
/// [`covering_source_range_for_project_duration`] for why core cannot call it.
fn frame_sample_position(frame: TimeCode, sample_rate: u32, fps: Rational) -> u128 {
    if frame.0 <= 0 {
        return 0;
    }
    u128::try_from(frame.0)
        .unwrap_or_default()
        .saturating_mul(u128::from(sample_rate))
        .saturating_mul(u128::from(fps.denominator()))
        / u128::from(fps.numerator())
}

/// The sample frames a gap of `project_duration` demands, rounded **up** so the
/// answer does not depend on where the gap sits on the timeline.
fn required_sample_frames(project_duration: TimeCode, sample_rate: u32, fps: Rational) -> u128 {
    if project_duration.0 <= 0 {
        return 0;
    }
    let numerator = u128::try_from(project_duration.0)
        .unwrap_or_default()
        .saturating_mul(u128::from(sample_rate))
        .saturating_mul(u128::from(fps.denominator()));
    let denominator = u128::from(fps.numerator());
    numerator.div_ceil(denominator)
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

        assert_eq!(
            map_frames(TimeCode(0), source, project).unwrap(),
            TimeCode(0)
        );
        assert_eq!(
            map_frames(TimeCode(24), source, project).unwrap(),
            TimeCode(30)
        );
        assert_eq!(
            map_frames(TimeCode(48), source, project).unwrap(),
            TimeCode(60)
        );
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
