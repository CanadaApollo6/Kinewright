//! Source coverage outside the requested cuts, matched by split ancestry.
use super::{BoundDialogue, DialogueMeasurements, Refusal, cached, request, spans};
use kinewright_core::{ClipId, Document, ExactMicros, TimeCode, TrackId, dialogue::AudibleSide};
use serde::Serialize;
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

type Interval = [ExactMicros; 2];
pub(crate) type Origins = BTreeMap<ClipId, ClipId>;
#[derive(Clone, Copy)]
pub(crate) struct Window {
    pub clip: ClipId,
    pub source: Interval,
    pub project: [TimeCode; 2],
}
#[derive(Default, Serialize)]
pub(crate) struct Changes {
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub measured: Vec<Change>,
    #[serde(skip_serializing_if = "zero")]
    pub unmeasured_extents: u64,
    #[serde(skip_serializing_if = "zero")]
    pub largest_unmeasured_us: u64,
    #[serde(skip)]
    pub positions: BTreeMap<TrackId, BTreeSet<TimeCode>>,
}
#[allow(clippy::trivially_copy_pass_by_ref, reason = "serde predicate")]
fn zero(v: &u64) -> bool {
    *v == 0
}
#[derive(Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum Change {
    QuietLoss {
        track: u64,
        clip: u64,
        source_us: Interval,
        micros: u64,
    },
    Growth {
        track: u64,
        clip: u64,
        source_us: Interval,
        micros: u64,
    },
    Padding {
        track: u64,
        clip: u64,
        before_us: ExactMicros,
        after_us: ExactMicros,
    },
}
pub(crate) struct Retention {
    before: Vec<super::check::PlayedSpan>,
    windows: Vec<Window>,
    by_origin: BTreeMap<ClipId, Vec<usize>>,
    target: TrackId,
    tracks: BTreeSet<TrackId>,
    one_frame: ExactMicros,
    exact_splits: bool,
    whole_multiple: bool,
}
fn time<T>(v: Result<T, kinewright_core::AudibleClockError>) -> Result<T, Refusal> {
    v.map_err(|e| request(e.to_string()))
}
fn length(span: Interval) -> Result<ExactMicros, Refusal> {
    time(span[1].checked_sub(span[0]))
}
fn difference(span: Interval, removed: &[Interval]) -> Vec<Interval> {
    let mut removed: Vec<_> = removed
        .iter()
        .copied()
        .filter(|r| r[0] < span[1] && span[0] < r[1])
        .collect();
    removed.sort_unstable_by_key(|r| (r[0], r[1]));
    let mut cursor = span[0];
    let mut kept = Vec::new();
    for r in removed {
        if cursor < r[0] {
            kept.push([cursor, r[0].min(span[1])]);
        }
        cursor = cursor.max(r[1]);
        if cursor >= span[1] {
            break;
        }
    }
    if cursor < span[1] {
        kept.push([cursor, span[1]]);
    }
    kept
}
impl Retention {
    pub(crate) fn new(
        doc: &Document,
        target: TrackId,
        windows: Vec<Window>,
    ) -> Result<Self, Refusal> {
        let tracks: BTreeSet<_> = doc
            .tracks
            .iter()
            .filter(|t| t.id == target || t.sync_lock)
            .map(|t| t.id)
            .collect();
        // Equal grids preserve source anchors. Integer multiples still need the
        // comparison: Core's inverse rounding may choose a different native phase.
        let whole_multiple = doc
            .tracks
            .iter()
            .filter(|t| tracks.contains(&t.id))
            .flat_map(|t| &t.clips)
            .all(|c| {
                if c.speed_percent != 100 || c.enabled_curve.is_some() {
                    return false;
                }
                doc.asset(c.asset).is_some_and(|a| {
                    let top = u64::from(a.fps.numerator()) * u64::from(doc.fps.denominator());
                    let bottom = u64::from(a.fps.denominator()) * u64::from(doc.fps.numerator());
                    top % bottom == 0
                })
            });
        let exact_splits = whole_multiple
            && doc
                .tracks
                .iter()
                .filter(|t| tracks.contains(&t.id))
                .flat_map(|t| &t.clips)
                .all(|c| doc.asset(c.asset).is_some_and(|a| a.fps == doc.fps));
        let before = if exact_splits {
            Vec::new()
        } else {
            spans(doc)?
                .into_iter()
                .filter(|s| tracks.contains(&s.track))
                .collect()
        };
        let mut by_origin: BTreeMap<ClipId, Vec<usize>> = BTreeMap::new();
        for (i, s) in before.iter().enumerate() {
            by_origin.entry(s.clip).or_default().push(i);
        }
        Ok(Self {
            before,
            by_origin,
            windows,
            target,
            tracks,
            one_frame: time(ExactMicros::frames(TimeCode(1), doc.fps))?,
            exact_splits,
            whole_multiple,
        })
    }
    fn allowed(
        &self,
        doc: &Document,
        base: &super::check::PlayedSpan,
    ) -> Result<Vec<Interval>, Refusal> {
        let mut allowed = Vec::new();
        for w in &self.windows {
            if base.track == self.target && base.clip == w.clip {
                allowed.push(w.source);
                continue;
            }
            let a = base.project.start.max(w.project[0]);
            let b = base.project.end.min(w.project[1]);
            if a >= b {
                continue;
            }
            let source = |at: TimeCode| -> Result<ExactMicros, Refusal> {
                Ok(time(base.source.start.checked_add(time(ExactMicros::frames(
                    TimeCode(at.0 - base.project.start.0),
                    doc.fps,
                ))?))?
                .min(base.source.end))
            };
            let r = [source(a)?, source(b)?];
            if r[0] < r[1] {
                allowed.push(r);
            }
        }
        Ok(allowed)
    }
    #[allow(
        clippy::too_many_lines,
        reason = "one comparison of retained source coverage, growth and outer padding"
    )]
    pub(crate) fn validate(
        &self,
        doc: &Document,
        origins: &Origins,
        values: &mut BTreeMap<kinewright_core::AssetId, Arc<BoundDialogue>>,
        provider: &dyn DialogueMeasurements,
    ) -> Result<Changes, Refusal> {
        if self.exact_splits {
            return Ok(Changes::default());
        }
        let after = spans(doc)?;
        let mut by_origin: BTreeMap<ClipId, Vec<_>> = BTreeMap::new();
        for s in &after {
            if self.tracks.contains(&s.track) {
                by_origin
                    .entry(*origins.get(&s.clip).unwrap_or(&s.clip))
                    .or_default()
                    .push(s);
            }
        }
        let mut changes = Changes::default();
        for base in &self.before {
            let after = by_origin.get(&base.clip).map_or(&[][..], Vec::as_slice);
            let actual: Vec<_> = after
                .iter()
                .map(|s| [s.source.start, s.source.end])
                .collect();
            let protected = difference(
                [base.source.start, base.source.end],
                &self.allowed(doc, base)?,
            );
            let losses: Vec<_> = protected
                .into_iter()
                .flat_map(|p| difference(p, &actual))
                .collect();
            if losses.is_empty() {
                continue;
            }
            let lost = losses.iter().try_fold(ExactMicros::ZERO, |sum, &r| {
                time(sum.checked_add(length(r)?))
            })?;
            let value = measurement(doc, base.asset, values, provider);
            if !base.proven || value.is_none() {
                for &r in &losses {
                    unknown(&mut changes, length(r)?);
                }
                continue;
            }
            let value = value.expect("checked measurement");
            if self.whole_multiple
                || lost > self.one_frame
                || losses.iter().any(|r| {
                    !value.dialogue.quiet_runs().iter().any(|q| {
                        ExactMicros::integer(q.start_micros()) <= r[0]
                            && r[1] <= ExactMicros::integer(q.end_micros())
                    })
                })
            {
                return Err(loss(base.clip, lost));
            }
            for &r in &losses {
                for s in after {
                    if s.source.end == r[0] {
                        guard(value.as_ref(), s, AudibleSide::Out, base.clip, lost)?;
                        positions(&mut changes, s, AudibleSide::Out);
                    }
                    if s.source.start == r[1] {
                        guard(value.as_ref(), s, AudibleSide::In, base.clip, lost)?;
                        positions(&mut changes, s, AudibleSide::In);
                    }
                }
                changes.measured.push(Change::QuietLoss {
                    track: base.track.0,
                    clip: base.clip.0,
                    source_us: r,
                    micros: length(r)?.ceil(),
                });
            }
        }
        for (&origin, parts) in &by_origin {
            let previous: Vec<_> = self
                .by_origin
                .get(&origin)
                .into_iter()
                .flatten()
                .map(|&i| &self.before[i])
                .collect();
            let old: Vec<_> = previous
                .iter()
                .map(|s| [s.source.start, s.source.end])
                .collect();
            for s in parts {
                let growth = difference([s.source.start, s.source.end], &old);
                let value = measurement(doc, s.asset, values, provider);
                for r in growth {
                    if self.whole_multiple {
                        return Err(loss(origin, length(r)?));
                    }
                    if !s.proven || value.is_none() {
                        unknown(&mut changes, length(r)?);
                        continue;
                    }
                    let value = value.as_ref().expect("checked measurement");
                    if r[0] == s.source.start {
                        value
                            .dialogue
                            .guard_exact(s.source.start, AudibleSide::In)
                            .map_err(Refusal::from)?;
                        positions(&mut changes, s, AudibleSide::In);
                    }
                    if r[1] == s.source.end {
                        value
                            .dialogue
                            .guard_exact(s.source.end, AudibleSide::Out)
                            .map_err(Refusal::from)?;
                        positions(&mut changes, s, AudibleSide::Out);
                    }
                    changes.measured.push(Change::Growth {
                        track: s.track.0,
                        clip: origin.0,
                        source_us: r,
                        micros: length(r)?.ceil(),
                    });
                }
                // Only the old outer tail owns pre-existing padding. New cut
                // boundaries carry their padding in the normal join readback.
                if let Some(base) = previous.iter().max_by_key(|b| b.source.end) {
                    let allowed = self.allowed(doc, base)?;
                    let outside = !allowed
                        .iter()
                        .any(|r| r[0] <= s.source.end && s.source.end <= r[1]);
                    if outside
                        && s.source.padding != base.source.padding
                        && parts
                            .last()
                            .is_some_and(|last| last.clip == s.clip && last.project == s.project)
                    {
                        let amount = if s.source.padding >= base.source.padding {
                            time(s.source.padding.checked_sub(base.source.padding))?
                        } else {
                            time(base.source.padding.checked_sub(s.source.padding))?
                        };
                        if self.whole_multiple {
                            return Err(loss(origin, amount));
                        }
                        if let Some(value) = value.as_ref().filter(|_| s.proven) {
                            value
                                .dialogue
                                .guard_exact(s.source.end, AudibleSide::Out)
                                .map_err(Refusal::from)?;
                            positions(&mut changes, s, AudibleSide::Out);
                            changes.measured.push(Change::Padding {
                                track: s.track.0,
                                clip: origin.0,
                                before_us: base.source.padding,
                                after_us: s.source.padding,
                            });
                        } else {
                            unknown(&mut changes, amount);
                        }
                    }
                }
            }
        }
        Ok(changes)
    }
    pub(crate) fn selected_edges(
        &self,
        doc: &Document,
        origins: &Origins,
        changes: &mut Changes,
    ) -> Result<(), Refusal> {
        let selected: BTreeSet<_> = self.windows.iter().map(|w| w.clip).collect();
        for s in spans(doc)? {
            if s.proven && selected.contains(origins.get(&s.clip).unwrap_or(&s.clip)) {
                positions(changes, &s, AudibleSide::In);
                positions(changes, &s, AudibleSide::Out);
            }
        }
        Ok(())
    }
}
fn positions(c: &mut Changes, s: &super::check::PlayedSpan, side: AudibleSide) {
    c.positions
        .entry(s.track)
        .or_default()
        .insert(if side == AudibleSide::Out {
            s.project.end
        } else {
            s.project.start
        });
}
fn unknown(c: &mut Changes, amount: ExactMicros) {
    c.unmeasured_extents += 1;
    c.largest_unmeasured_us = c.largest_unmeasured_us.max(amount.ceil());
}
fn loss(clip: ClipId, amount: ExactMicros) -> Refusal {
    Refusal::UnselectedAudio{clip:clip.0,lost_micros:amount.ceil(),why:"frame rates differ; unselected audio is not proven quiet within one project frame; choose another cut alignment".into()}
}
fn guard(
    value: &BoundDialogue,
    s: &super::check::PlayedSpan,
    side: AudibleSide,
    clip: ClipId,
    lost: ExactMicros,
) -> Result<(), Refusal> {
    if !s.proven {
        return Err(Refusal::UnprovenSegment { clip: s.clip.0 });
    }
    value
        .dialogue
        .guard_exact(
            if side == AudibleSide::Out {
                s.source.end
            } else {
                s.source.start
            },
            side,
        )
        .map_err(|_| loss(clip, lost))
}
fn measurement(
    doc: &Document,
    asset: kinewright_core::AssetId,
    values: &mut BTreeMap<kinewright_core::AssetId, Arc<BoundDialogue>>,
    provider: &dyn DialogueMeasurements,
) -> Option<Arc<BoundDialogue>> {
    if !values.contains_key(&asset) {
        let value = cached(provider, doc.asset(asset)?, values).ok()?;
        values.insert(asset, value);
    }
    values.get(&asset).map(Arc::clone)
}
