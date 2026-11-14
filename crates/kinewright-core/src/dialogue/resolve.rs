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
#[derive(Debug, PartialEq, Eq, Serialize)]
pub struct ResolvedCut {
    pub start: ResolvedEdge,
    pub end: ResolvedEdge,
    pub request: CutRequest,
}

#[derive(Debug, PartialEq, Eq, Serialize, Error)]
#[serde(tag = "code", rename_all = "snake_case")]
pub enum RefusalReason {
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
        let dip = self
            .profile
            .hop(index)
            .expect("validated index")
            .rms_dbfs_hundredths
            <= self.settings.dip_dbfs_hundredths;
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
        let n = u128::from(at_micros);
        if at_micros == 0 || at_micros == self.profile.duration_micros() {
            return None;
        }
        let index = self.profile.index_at(n, 1)?;
        if self.profile.hop(index)?.rms_dbfs_hundredths > self.settings.dip_dbfs_hundredths {
            return None;
        }
        let (word, boundary, depth) = self.deepest_intrusion(n, 1)?;
        Some(DipIntrusion {
            word,
            boundary,
            // Denominator one bounds depth by the u64 source duration.
            micros: u64::try_from(depth).ok()?,
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
            .filter(|(_, _, db)| *db > self.settings.quiet_rms_max)
            .map_or(fallback, |(at, _, _)| at)
    }
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
                        run.start_micros < old.start_micros
                    } else {
                        run.end_micros > old.end_micros
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
            *db <= self.settings.dip_dbfs_hundredths && self.guard(*at, side).is_ok()
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
    /// Resolve word-addressed intent without executing an edit or guessing an
    /// unmeasured edge. The same input produces identical integer evidence.
    ///
    /// # Errors
    /// Refuses invalid references, absent quiet, two dip edges, or unsafe ranges.
    pub fn resolve(&self, request: CutRequest) -> Result<ResolvedCut, Refusal> {
        let (start, end) = match request {
            CutRequest::RemoveWords { from, to } => {
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
                if a.at_micros >= first.end_micros || b.at_micros <= last.start_micros {
                    return Err(self.refuse(RefusalReason::NotForward, a.at_micros));
                }
                (a, b)
            }
            CutRequest::TrimPause {
                after_word,
                leave_micros,
            } => self.pause_edges(after_word, leave_micros)?,
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
        Ok(ResolvedCut {
            start,
            end,
            request,
        })
    }
    fn pause_edges(
        &self,
        index: usize,
        leave: u64,
    ) -> Result<(ResolvedEdge, ResolvedEdge), Refusal> {
        let Some(before) = self.words.get(index) else {
            return Err(self.refuse(RefusalReason::NoFollowingWord, 0));
        };
        let Some(after) = self.words.get(index + 1) else {
            return Err(self.refuse(RefusalReason::NoFollowingWord, before.end_micros));
        };
        if self.gaps.is_empty() {
            return Err(self.refuse(RefusalReason::NoQuietEvidence, before.end_micros));
        }
        if after.start_micros.saturating_sub(before.end_micros) <= leave {
            return Err(self.refuse(RefusalReason::NothingToTrim, before.end_micros));
        }
        let total = self.settings.air_after_micros + self.settings.air_before_micros;
        let share = u64::try_from(
            u128::from(leave) * u128::from(self.settings.air_after_micros) / u128::from(total),
        )
        .map_err(|_| self.refuse(RefusalReason::TimeOverflow, 0))?;
        let start = before
            .end_micros
            .checked_add(share)
            .ok_or_else(|| self.refuse(RefusalReason::TimeOverflow, before.end_micros))?;
        let end = after
            .start_micros
            .checked_sub(leave - share)
            .ok_or_else(|| self.refuse(RefusalReason::NothingToTrim, after.start_micros))?;
        let mut first = None;
        let mut last = None;
        for &run in &self.gaps {
            if run.end_micros
                <= before
                    .end_micros
                    .saturating_sub(self.settings.kept_slack_micros)
                || run.start_micros
                    >= after
                        .start_micros
                        .saturating_add(self.settings.kept_slack_micros)
                || run.duration() < 2 * self.settings.quiet_margin_micros
            {
                continue;
            }
            if first.is_none_or(|old: Span| run.start_micros < old.start_micros) {
                first = Some(run);
            }
            if last.is_none_or(|old: Span| run.end_micros > old.end_micros) {
                last = Some(run);
            }
        }
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
        let a = place(first, start, AudibleSide::Out)?;
        let b = place(last, end, AudibleSide::In)?;
        if b.at_micros.saturating_sub(a.at_micros) < self.settings.minimum_pause_cut_micros {
            return Err(self.refuse(RefusalReason::NothingToTrim, a.at_micros));
        }
        Ok((a, b))
    }
}
