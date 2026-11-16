// Derived from Scarlet packages/mcp/src/dialogue-join.ts and dialogue-plan.ts at 2cbab542; see LICENSES/scarlet-MIT.txt.
use super::{Dialogue, InputError, Settings, Span, Word};
use serde::Serialize;
use thiserror::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AudibleSide {
    In,
    Out,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WordBoundary {
    Start,
    End,
}
/// The deepest word-timing intrusion at a measured dip, without copied text.
/// This measurement is not permission to use the edge.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct DipIntrusion {
    pub word: usize,
    pub boundary: WordBoundary,
    pub micros: u64,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CutRequest {
    RemoveWords {
        from: usize,
        to: usize,
        leave_micros: Option<u64>,
    },
    TrimPause {
        after_word: usize,
        leave_micros: u64,
    },
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Shift {
    pub micros: u64,
    pub later: bool,
}
impl Shift {
    fn between(from: u64, to: u64) -> Self {
        Self {
            micros: from.abs_diff(to),
            later: to > from,
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum EdgePlacement {
    Quiet {
        run: Span,
        air_after_micros: u64,
        air_before_micros: u64,
        shift: Shift,
        snapped: bool,
    },
    LevelDip {
        hop: Span,
        dip_run: Span,
        rms_dbfs_hundredths: i16,
        shift: Shift,
        intrusion: Option<DipIntrusion>,
    },
    SourceBoundary {
        at_start: bool,
    },
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Support {
    pub start: u64,
    pub end: u64,
    pub inclusive_end: bool,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct ResolvedEdge {
    pub(super) at_micros: u64,
    pub(super) side: AudibleSide,
    pub(super) placement: EdgePlacement,
    #[serde(skip)]
    pub(super) support: Support,
}
impl ResolvedEdge {
    #[must_use]
    pub const fn at_micros(self) -> u64 {
        self.at_micros
    }
    #[must_use]
    pub const fn side(self) -> AudibleSide {
        self.side
    }
    #[must_use]
    pub const fn placement(self) -> EdgePlacement {
        self.placement
    }
    pub(super) const fn is_dip(self) -> bool {
        matches!(self.placement, EdgePlacement::LevelDip { .. })
    }
    fn quiet_run(self) -> Option<Span> {
        match self.placement {
            EdgePlacement::Quiet { run, .. } => Some(run),
            _ => None,
        }
    }
}
/// Measured quiet supporting the actual join, before frame rounding.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct RetainedAir {
    pub left: Option<Span>,
    pub right: Option<Span>,
    pub actual_micros: u64,
}
#[derive(Debug, PartialEq, Eq, Serialize)]
pub struct ResolvedCut {
    pub air: Option<RetainedAir>,
    pub other_pauses_micros: Vec<u64>,
    pub start: ResolvedEdge,
    pub end: ResolvedEdge,
    pub request: CutRequest,
}

#[derive(Debug, PartialEq, Eq, Serialize, Error)]
#[serde(tag = "code", rename_all = "snake_case")]
pub enum RefusalReason {
    #[error("{issue}")]
    Selection {
        #[from]
        #[serde(flatten)]
        issue: super::SelectionIssue,
    },
    #[error("word selection is outside this recording's ordered words")]
    WordRange,
    #[error("no following word names a pause to trim")]
    NoFollowingWord,
    #[error("the recording contains no measured quiet pause; no edge was guessed")]
    NoQuietEvidence,
    #[error("no measured quiet or sufficiently deep dip brackets this word boundary")]
    NoMeasuredEdge,
    #[error("both edges are dips between run-together words; audition this removal")]
    BothDipEdges,
    #[error("the measured edges do not bracket one forward removal")]
    NotForward,
    #[error(
        "requested air {requested_micros} microseconds is below the two quiet margins ({minimum_micros})"
    )]
    AirBelowGuardMargin {
        requested_micros: u64,
        minimum_micros: u64,
    },
    #[error("the pause is already too short to leave the requested duration")]
    NothingToTrim,
    #[error(
        "measured word removal is {actual_micros} microseconds; at least {minimum_micros} are required"
    )]
    WordCutTooShort {
        actual_micros: u64,
        minimum_micros: u64,
    },
    #[error("edge intersects sounding word #{index} {text:?}")]
    SoundingWord { index: usize, text: String },
    #[error("edge intersects sounding untranscribed audio")]
    UntranscribedSound,
    #[error("edge has no measurement on this recording's source clock")]
    Unmeasured,
    #[error("no project frame lies inside this edge's measured support")]
    NoFrameInRun,
    #[error("the source time or frame cannot be represented")]
    TimeOverflow,
    #[error("frame rate must be a positive rational")]
    FrameRate,
}
#[derive(Debug, PartialEq, Eq, Serialize, Error)]
#[error(
    "{reason}; source microseconds {at_micros}, quiet before {quiet_before:?}, quiet after {quiet_after:?}"
)]
pub struct Refusal {
    pub reason: RefusalReason,
    pub at_micros: u64,
    pub quiet_before: Option<u64>,
    pub quiet_after: Option<u64>,
}

/// Leave the requested air inside a measured run, splitting short runs
/// proportionally, with an interior safety margin on both sides.
///
/// # Errors
/// Refuses invalid settings or a run without a trusted interior.
pub fn pad_edge(run: Span, side: AudibleSide, settings: &Settings) -> Result<u64, InputError> {
    settings.validate()?;
    let length = run
        .end_micros
        .checked_sub(run.start_micros)
        .ok_or(InputError::Span)?;
    let margin = settings.quiet_margin_micros;
    if length < 2 * margin {
        return Err(InputError::Span);
    }
    let total = settings.air_after_micros + settings.air_before_micros;
    let target = if length < total {
        run.start_micros
            + u64::try_from(
                u128::from(length) * u128::from(settings.air_after_micros) / u128::from(total),
            )
            .map_err(|_| InputError::Span)?
    } else if side == AudibleSide::Out {
        run.start_micros + settings.air_after_micros
    } else {
        run.end_micros - settings.air_before_micros
    };
    Ok(target.clamp(run.start_micros + margin, run.end_micros - margin))
}

impl Dialogue {
    pub(super) fn refuse(&self, reason: RefusalReason, at: u64) -> Refusal {
        let (quiet_before, quiet_after) = self.nearest_quiet(at);
        Refusal {
            reason,
            at_micros: at,
            quiet_before,
            quiet_after,
        }
    }
    /// Guard a newly audible source edge, including sound omitted by the transcript.
    /// Measured long quiet overrides stretched word timings. Inside a word,
    /// a deep dip is usable only within the configured boundary reach.
    ///
    /// # Errors
    /// Returns a named sounding word, untranscribed sound, or missing measurement,
    /// always with the closest trusted quiet point on either side when one exists.
    pub fn guard(&self, at_micros: u64, side: AudibleSide) -> Result<(), Refusal> {
        self.guard_ratio(u128::from(at_micros), 1, side)
    }
    /// Guard an audible edge without discarding its sub-microsecond fraction.
    /// # Errors
    /// Returns the same typed acoustic refusal as `guard` at the exact instant.
    pub fn guard_exact(&self, at: crate::ExactMicros, side: AudibleSide) -> Result<(), Refusal> {
        self.guard_ratio(at.numerator(), u128::from(at.denominator()), side)
    }
    pub(super) fn guard_ratio(&self, n: u128, d: u128, _side: AudibleSide) -> Result<(), Refusal> {
        let at = u64::try_from(n / d).map_err(|_| self.refuse(RefusalReason::TimeOverflow, 0))?;
        if n == 0 || n == u128::from(self.profile.duration_micros()) * d {
            return Ok(());
        }
        let index = self
            .profile
            .index_at(n, d)
            .ok_or_else(|| self.refuse(RefusalReason::Unmeasured, at))?;
        if self.quiet_at_ratio(n, d) {
            return Ok(());
        }
        // A known gap must supply its guarded interior. Reclassifying its
        // margin as a dip would bypass the mandatory quiet safety margin.
        let outside_gap_margin = self.gaps.iter().any(|run| {
            let margin = self.settings.quiet_margin_micros;
            u128::from(run.start_micros) * d <= n
                && n < u128::from(run.end_micros) * d
                && !(run.duration() >= margin * 2
                    && u128::from(run.start_micros + margin) * d <= n
                    && n <= u128::from(run.end_micros - margin) * d)
        });
        let dip = !outside_gap_margin
            && self.thresholds.dip_dbfs_hundredths.is_some_and(|limit| {
                self.profile
                    .hop(index)
                    .expect("validated index")
                    .rms_dbfs_hundredths
                    <= limit
            });
        if let Some((index, _, depth)) = self.deepest_intrusion(n, d) {
            if dip && depth <= u128::from(self.settings.dip_reach_micros) * d {
                return Ok(());
            }
            let word = &self.words[index];
            // The error owns the named word so it can outlive the analysis.
            return Err(self.refuse(
                RefusalReason::SoundingWord {
                    index,
                    text: word.text.clone(),
                },
                at,
            ));
        }
        if dip {
            return Ok(());
        }
        Err(self.refuse(RefusalReason::UntranscribedSound, at))
    }
    fn deepest_intrusion(&self, n: u128, d: u128) -> Option<(usize, WordBoundary, u128)> {
        let first = self
            .words
            .partition_point(|w| u128::from(w.end_micros) * d <= n);
        self.words[first..]
            .iter()
            .enumerate()
            .take_while(|(_, w)| u128::from(w.start_micros) * d < n)
            .filter(|(_, w)| {
                w.is_spoken()
                    && u128::from(
                        w.start_micros
                            .saturating_add(self.settings.word_margin_micros),
                    ) * d
                        < n
                    && n < u128::from(
                        w.end_micros
                            .saturating_sub(self.settings.word_margin_micros),
                    ) * d
            })
            .map(|(i, w)| {
                let from_start = n - u128::from(w.start_micros) * d;
                let from_end = u128::from(w.end_micros) * d - n;
                if from_start <= from_end {
                    (first + i, WordBoundary::Start, from_start)
                } else {
                    (first + i, WordBoundary::End, from_end)
                }
            })
            .max_by(|a, b| a.2.cmp(&b.2).then_with(|| b.0.cmp(&a.0)))
    }
    /// Measure a dip's word-timing intrusion at an integer source instant.
    /// The guard independently decides whether it is usable. With overlapping
    /// words the deepest intrusion wins; a quiet stretched timing can also be
    /// reported without changing the guard's trusted-quiet rule.
    #[must_use]
    pub fn dip_intrusion(&self, at_micros: u64) -> Option<DipIntrusion> {
        self.dip_intrusion_exact(crate::ExactMicros::integer(at_micros))
    }
    /// Intrusion at a fractional audible edge, conservatively rounded up to a
    /// whole microsecond in the warning. The guard uses the exact fraction.
    #[must_use]
    pub fn dip_intrusion_exact(&self, at: crate::ExactMicros) -> Option<DipIntrusion> {
        let n = at.numerator();
        let d = u128::from(at.denominator());
        if n == 0 || n == u128::from(self.profile.duration_micros()) * d {
            return None;
        }
        let index = self.profile.index_at(n, d)?;
        if self.profile.hop(index)?.rms_dbfs_hundredths > self.thresholds.dip_dbfs_hundredths? {
            return None;
        }
        let (word, boundary, depth) = self.deepest_intrusion(n, d)?;
        Some(DipIntrusion {
            word,
            boundary,
            micros: u64::try_from(depth.div_ceil(d)).ok()?,
        })
    }
    fn boundary(&self, start: bool) -> ResolvedEdge {
        let at = if start {
            0
        } else {
            self.profile.duration_micros()
        };
        ResolvedEdge {
            at_micros: at,
            side: if start {
                AudibleSide::Out
            } else {
                AudibleSide::In
            },
            placement: EdgePlacement::SourceBoundary { at_start: start },
            support: Support {
                start: at,
                end: at,
                inclusive_end: true,
            },
        }
    }
    fn quiet_edge(
        &self,
        run: Span,
        side: AudibleSide,
        at: u64,
        nominal: u64,
    ) -> Option<ResolvedEdge> {
        let (start, end) = self.quiet_support(run, at)?;
        let shift = Shift::between(nominal, at);
        Some(ResolvedEdge {
            at_micros: at,
            side,
            placement: EdgePlacement::Quiet {
                run,
                air_after_micros: at - run.start_micros,
                air_before_micros: run.end_micros - at,
                shift,
                snapped: shift.micros <= self.settings.snap_limit_micros,
            },
            support: Support {
                start,
                end,
                inclusive_end: true,
            },
        })
    }
    fn extreme(&self, from: u64, to: u64, highest: bool) -> Option<(u64, usize, i16)> {
        if from > to {
            return None;
        }
        let hop = u64::from(self.profile.hop_micros());
        let first = usize::try_from(from / hop).ok()?;
        let mut best: Option<(u64, usize, i16)> = None;
        for index in first..self.profile.len() {
            let span = self.profile.span(index);
            let at = span.start_micros + span.duration() / 2;
            if at > to {
                break;
            }
            if at < from {
                continue;
            }
            let level = self.profile.hop(index)?.rms_dbfs_hundredths;
            if best.is_none_or(|(_, _, db)| if highest { level > db } else { level < db }) {
                best = Some((at, index, level));
            }
        }
        best
    }
    fn anchor(&self, word: &Word, fallback: u64) -> u64 {
        self.extreme(word.start_micros, word.end_micros, true)
            .filter(|(_, _, db)| *db > self.thresholds.quiet_rms_max)
            .map_or(fallback, |(at, _, _)| at)
    }
    #[allow(
        clippy::too_many_lines,
        reason = "one boundary search keeps quiet and dip evidence on the same source clock"
    )]
    fn word_edge(
        &self,
        kept: Option<&Word>,
        removed: &Word,
        side: AudibleSide,
        allow_gap: bool,
    ) -> Result<ResolvedEdge, Refusal> {
        let Some(kept) = kept else {
            return Ok(self.boundary(side == AudibleSide::Out));
        };
        let start = side == AudibleSide::Out;
        let (lo, hi) = if start {
            (kept.end_micros, removed.start_micros)
        } else {
            (removed.end_micros, kept.start_micros)
        };
        let nominal = if start {
            removed.start_micros
        } else {
            removed.end_micros
        };
        let (left, right) = if start {
            (kept, removed)
        } else {
            (removed, kept)
        };
        let a = self.anchor(left, left.start_micros);
        let b = self.anchor(right, right.end_micros);
        let mut best: Option<ResolvedEdge> = None;
        if allow_gap {
            for &run in &self.gaps {
                let fits = if start {
                    run.end_micros > lo.saturating_sub(self.settings.kept_slack_micros)
                        && run.start_micros < hi.saturating_add(self.settings.removed_slack_micros)
                } else {
                    run.start_micros < hi.saturating_add(self.settings.kept_slack_micros)
                        && run.end_micros > lo.saturating_sub(self.settings.removed_slack_micros)
                };
                if !fits {
                    continue;
                }
                let Ok(at) = pad_edge(run, side, &self.settings) else {
                    continue;
                };
                if !(a < at && at < b) {
                    continue;
                }
                let Some(edge) = self.quiet_edge(run, side, at, nominal) else {
                    continue;
                };
                if best.is_none_or(|old| {
                    let old = old.quiet_run().expect("quiet edge");
                    if start {
                        run.start_micros > old.start_micros
                    } else {
                        run.end_micros < old.end_micros
                    }
                }) {
                    best = Some(edge);
                }
            }
        }
        if let Some(edge) = best {
            return Ok(edge);
        }
        let from = a.max(if start {
            kept.end_micros
                .saturating_sub(self.settings.dip_reach_micros)
        } else {
            lo.min(hi).saturating_sub(self.settings.kept_slack_micros)
        });
        let to = b.min(if start {
            lo.max(hi).saturating_add(self.settings.kept_slack_micros)
        } else {
            kept.start_micros
                .saturating_add(self.settings.dip_reach_micros)
        });
        if let Some((at, index, db)) = self.extreme(from, to, false).filter(|(at, _, db)| {
            self.thresholds
                .dip_dbfs_hundredths
                .is_some_and(|limit| *db <= limit)
                && self.guard(*at, side).is_ok()
        }) {
            let hop = self.profile.span(index);
            let dip_run = self
                .dips
                .iter()
                .copied()
                .find(|run| run.start_micros <= at && at < run.end_micros)
                .expect("the minimum belongs to a prepared dip run");
            return Ok(ResolvedEdge {
                at_micros: at,
                side,
                placement: EdgePlacement::LevelDip {
                    hop,
                    dip_run,
                    rms_dbfs_hundredths: db,
                    shift: Shift::between(nominal, at),
                    intrusion: self.dip_intrusion(at),
                },
                support: Support {
                    start: dip_run.start_micros.max(from),
                    end: dip_run.end_micros.min(to),
                    inclusive_end: to < dip_run.end_micros,
                },
            });
        }
        Err(self.refuse(RefusalReason::NoMeasuredEdge, nominal))
    }
    fn paired_air(
        &self,
        a: &mut ResolvedEdge,
        b: &mut ResolvedEdge,
        runs: [Span; 2],
        nominal: [u64; 2],
        leave_micros: Option<u64>,
    ) -> Result<RetainedAir, Refusal> {
        let [left, right] = runs;
        let margin = self.settings.quiet_margin_micros;
        if let Some(requested_micros) = leave_micros
            && requested_micros < margin * 2
        {
            return Err(self.refuse(
                RefusalReason::AirBelowGuardMargin {
                    requested_micros,
                    minimum_micros: margin * 2,
                },
                a.at_micros,
            ));
        }
        let target = leave_micros.unwrap_or_else(|| left.duration().max(right.duration()));
        let total = self.settings.air_after_micros + self.settings.air_before_micros;
        let preferred = if leave_micros.is_some() {
            u64::try_from(
                u128::from(target) * u128::from(self.settings.air_after_micros) / u128::from(total),
            )
            .map_err(|_| self.refuse(RefusalReason::TimeOverflow, a.at_micros))?
        } else if left.duration() >= right.duration() {
            left.duration() - margin
        } else {
            margin
        };
        let left_max = left.duration() - margin;
        let right_max = right.duration() - margin;
        let target = target
            .min(left_max.saturating_add(right_max))
            .max(margin * 2);
        let mut left_air = preferred.clamp(margin, left_max);
        let right_air = target.saturating_sub(left_air).clamp(margin, right_max);
        left_air = target.saturating_sub(right_air).clamp(margin, left_max);
        *a = self
            .quiet_edge(
                left,
                AudibleSide::Out,
                left.start_micros + left_air,
                nominal[0],
            )
            .ok_or_else(|| self.refuse(RefusalReason::NoMeasuredEdge, a.at_micros))?;
        *b = self
            .quiet_edge(
                right,
                AudibleSide::In,
                right.end_micros - right_air,
                nominal[1],
            )
            .ok_or_else(|| self.refuse(RefusalReason::NoMeasuredEdge, b.at_micros))?;
        Ok(RetainedAir {
            left: Some(left),
            right: Some(right),
            actual_micros: left_air + right_air,
        })
    }
    fn word_air(
        &self,
        a: &mut ResolvedEdge,
        b: &mut ResolvedEdge,
        first: &Word,
        last: &Word,
        leave_micros: Option<u64>,
    ) -> Result<Option<RetainedAir>, Refusal> {
        let left = a.quiet_run();
        let right = b.quiet_run();
        if let (Some(l), Some(r)) = (left, right)
            && l != r
        {
            return self
                .paired_air(
                    a,
                    b,
                    [l, r],
                    [first.start_micros, last.end_micros],
                    leave_micros,
                )
                .map(Some);
        }
        if let Some(run) = left.or(right)
            && left.is_some() != right.is_some()
        {
            let margin = self.settings.quiet_margin_micros;
            // Natural-pause preservation needs two measured flanks.
            // A boundary or dip on the other side keeps standard air.
            let leave = leave_micros.unwrap_or_else(|| {
                if left.is_some() {
                    self.settings.air_after_micros
                } else {
                    self.settings.air_before_micros
                }
            });
            let leave = leave.clamp(margin, run.duration() - margin);
            if left.is_some() {
                let at = run.start_micros + leave;
                *a = self
                    .quiet_edge(run, AudibleSide::Out, at, first.start_micros)
                    .ok_or_else(|| self.refuse(RefusalReason::NoMeasuredEdge, at))?;
            } else {
                let at = run.end_micros - leave;
                *b = self
                    .quiet_edge(run, AudibleSide::In, at, last.end_micros)
                    .ok_or_else(|| self.refuse(RefusalReason::NoMeasuredEdge, at))?;
            }
            return Ok(Some(RetainedAir {
                left,
                right,
                actual_micros: leave,
            }));
        }
        Ok(None)
    }
    /// Resolve word-addressed intent without executing an edit or guessing an
    /// unmeasured edge. The same input produces identical integer evidence.
    ///
    /// # Errors
    /// Refuses invalid references, absent quiet, two dip edges, or unsafe ranges.
    pub fn resolve(&self, request: CutRequest) -> Result<ResolvedCut, Refusal> {
        self.resolve_available(request, None)
    }
    /// Trim a pause using only the source span still audible in this clip.
    /// This lets a later call select a remaining run after an earlier split.
    /// # Errors
    /// Refuses unavailable source time or the same measured-edge failures as `resolve`.
    pub fn resolve_pause_in(
        &self,
        after_word: usize,
        leave_micros: u64,
        available: Span,
    ) -> Result<ResolvedCut, Refusal> {
        if available.end_micros > self.profile.duration_micros() {
            return Err(self.refuse(RefusalReason::Unmeasured, available.end_micros));
        }
        self.resolve_available(
            CutRequest::TrimPause {
                after_word,
                leave_micros,
            },
            Some(available),
        )
    }
    fn resolve_available(
        &self,
        request: CutRequest,
        available: Option<Span>,
    ) -> Result<ResolvedCut, Refusal> {
        let mut other_pauses_micros = Vec::new();
        let mut air = None;
        let (start, end) = match request {
            CutRequest::RemoveWords {
                from,
                to,
                leave_micros,
            } => {
                if from > to || to >= self.words.len() {
                    return Err(self.refuse(RefusalReason::WordRange, 0));
                }
                if self.gaps.is_empty() {
                    return Err(self.refuse(
                        RefusalReason::NoQuietEvidence,
                        self.words[from].start_micros,
                    ));
                }
                let first = &self.words[from];
                let last = &self.words[to];
                let mut a = self.word_edge(
                    from.checked_sub(1).map(|i| &self.words[i]),
                    first,
                    AudibleSide::Out,
                    true,
                )?;
                let mut b = self.word_edge(self.words.get(to + 1), last, AudibleSide::In, true)?;
                if let (Some(left), Some(right)) = (a.quiet_run(), b.quiet_run())
                    && left == right
                {
                    let run_mid = u128::from(left.start_micros) + u128::from(left.end_micros);
                    let word_mid = u128::from(first.start_micros) + u128::from(last.end_micros);
                    if run_mid < word_mid {
                        b = self.word_edge(self.words.get(to + 1), last, AudibleSide::In, false)?;
                    } else {
                        a = self.word_edge(
                            from.checked_sub(1).map(|i| &self.words[i]),
                            first,
                            AudibleSide::Out,
                            false,
                        )?;
                    }
                }
                air = self.word_air(&mut a, &mut b, first, last, leave_micros)?;
                if a.at_micros >= first.end_micros || b.at_micros <= last.start_micros {
                    return Err(self.refuse(RefusalReason::NotForward, a.at_micros));
                }
                (a, b)
            }
            CutRequest::TrimPause {
                after_word,
                leave_micros,
            } => {
                let (a, b, other) = self.pause_edges(after_word, leave_micros, available)?;
                other_pauses_micros = other;
                if let Some(run) = a.quiet_run() {
                    air = Some(RetainedAir {
                        left: Some(run),
                        right: Some(run),
                        actual_micros: run.duration() - (b.at_micros - a.at_micros),
                    });
                }
                (a, b)
            }
        };
        if start.is_dip() && end.is_dip() {
            return Err(self.refuse(RefusalReason::BothDipEdges, start.at_micros));
        }
        if start.at_micros >= end.at_micros {
            return Err(self.refuse(RefusalReason::NotForward, start.at_micros));
        }
        if matches!(request, CutRequest::RemoveWords { .. })
            && end.at_micros - start.at_micros < self.settings.minimum_word_cut_micros
        {
            return Err(self.refuse(
                RefusalReason::WordCutTooShort {
                    actual_micros: end.at_micros - start.at_micros,
                    minimum_micros: self.settings.minimum_word_cut_micros,
                },
                start.at_micros,
            ));
        }
        self.guard(start.at_micros, start.side)?;
        self.guard(end.at_micros, end.side)?;
        self.validate_cut(
            request,
            [
                crate::ExactMicros::integer(start.at_micros),
                crate::ExactMicros::integer(end.at_micros),
            ],
        )?;
        Ok(ResolvedCut {
            air,
            other_pauses_micros,
            start,
            end,
            request,
        })
    }
    #[allow(
        clippy::too_many_lines,
        reason = "one pause selection measures full and available runs before placing its two edges"
    )]
    fn pause_edges(
        &self,
        index: usize,
        leave: u64,
        available: Option<Span>,
    ) -> Result<(ResolvedEdge, ResolvedEdge, Vec<u64>), Refusal> {
        let Some(before) = self.words.get(index) else {
            return Err(self.refuse(RefusalReason::NoFollowingWord, 0));
        };
        let Some(after) = self.words.get(index + 1) else {
            return Err(self.refuse(RefusalReason::NoFollowingWord, before.end_micros));
        };
        if self.gaps.is_empty() {
            return Err(self.refuse(RefusalReason::NoQuietEvidence, before.end_micros));
        }
        // Word starts locate the exchange; measured run bounds determine its
        // air. A stretched ASR end must not hide the silence inside that word.
        let mut runs: Vec<Span> = Vec::new();
        let mut full_micros = 0;
        let mut usable_micros = 0;
        for &run in &self.gaps {
            if run.start_micros < before.start_micros || run.start_micros >= after.start_micros {
                continue;
            }
            full_micros = full_micros.max(run.duration());
            let run = if let Some(span) = available {
                let start = run.start_micros.max(span.start_micros);
                let end = run.end_micros.min(span.end_micros);
                if start >= end {
                    continue;
                }
                Span {
                    start_micros: start,
                    end_micros: end,
                }
            } else {
                run
            };
            usable_micros = usable_micros.max(run.duration());
            if run.duration() < 2 * self.settings.quiet_margin_micros {
                continue;
            }
            if let Some(last) = runs
                .last_mut()
                .filter(|last| run.start_micros <= last.end_micros)
            {
                last.end_micros = last.end_micros.max(run.end_micros);
            } else {
                runs.push(run);
            }
        }
        if let Some(available) = available
            && full_micros > leave
            && usable_micros <= leave
            && usable_micros < full_micros
        {
            return Err(self.refuse(
                super::SelectionIssue::PauseLimited {
                    full_micros,
                    usable_micros,
                    leave_micros: leave,
                    available,
                }
                .into(),
                before.start_micros,
            ));
        }
        if !runs.is_empty() && runs.iter().all(|run| run.duration() <= leave) {
            return Err(self.refuse(RefusalReason::NothingToTrim, before.start_micros));
        }
        let longest = runs
            .iter()
            .enumerate()
            .max_by_key(|(_, run)| (run.duration(), std::cmp::Reverse(run.start_micros)))
            .map(|(index, _)| index)
            .ok_or_else(|| self.refuse(RefusalReason::NoMeasuredEdge, before.start_micros))?;
        let run = runs[longest];
        let other = runs
            .iter()
            .enumerate()
            .filter(|(i, _)| *i != longest)
            .map(|(_, run)| run.duration())
            .collect();
        if run.duration() <= leave {
            return Err(self.refuse(RefusalReason::NothingToTrim, run.start_micros));
        }
        let total = self.settings.air_after_micros + self.settings.air_before_micros;
        let share = u64::try_from(
            u128::from(leave) * u128::from(self.settings.air_after_micros) / u128::from(total),
        )
        .map_err(|_| self.refuse(RefusalReason::TimeOverflow, run.start_micros))?;
        let start = run
            .start_micros
            .checked_add(share)
            .ok_or_else(|| self.refuse(RefusalReason::TimeOverflow, run.start_micros))?;
        let end = run
            .end_micros
            .checked_sub(leave - share)
            .ok_or_else(|| self.refuse(RefusalReason::NothingToTrim, run.end_micros))?;
        let place = |run: Option<Span>, at: u64, side| {
            let run = run.ok_or_else(|| self.refuse(RefusalReason::NoMeasuredEdge, at))?;
            let margin = self.settings.quiet_margin_micros;
            self.quiet_edge(
                run,
                side,
                at.clamp(run.start_micros + margin, run.end_micros - margin),
                at,
            )
            .ok_or_else(|| self.refuse(RefusalReason::NoMeasuredEdge, at))
        };
        let a = place(Some(run), start, AudibleSide::Out)?;
        let b = place(Some(run), end, AudibleSide::In)?;
        if b.at_micros.saturating_sub(a.at_micros) < self.settings.minimum_pause_cut_micros {
            return Err(self.refuse(RefusalReason::NothingToTrim, a.at_micros));
        }
        Ok((a, b, other))
    }
}
