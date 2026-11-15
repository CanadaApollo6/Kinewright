use super::{BoundDialogue, DialogueMeasurements, Refusal, cached, clip_end, request, spans};
use kinewright_core::{
    ClipId, Document, ExactMicros, Operation, TimeCode, TimelineRevision, TrackId,
    dialogue::{AudibleSide, CutRequest},
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, sync::Arc};

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct CutArgs {
    pub expected_revision: TimelineRevision,
    pub removals: Vec<Removal>,
}
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct WordAddress {
    pub clip: u64,
    pub index: usize,
    pub binding: String,
}
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum Removal {
    Words {
        first: WordAddress,
        last: WordAddress,
        overrule: Option<[u64; 2]>,
    },
    Pause {
        after: WordAddress,
        leave_micros: u64,
        overrule: Option<[u64; 2]>,
    },
}
/// [committed project frame, recording/asset id, exact audible source microseconds, placement,
/// optional dip [word index, boundary, intrusion microseconds]].
#[derive(Debug, Serialize)]
pub(crate) struct Edge(
    pub i64,
    pub u64,
    pub ExactMicros,
    pub &'static str,
    #[serde(skip_serializing_if = "Option::is_none")] pub Option<(usize, &'static str, u64)>,
);
#[derive(Debug, Serialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub(crate) enum Outcome {
    Ready {
        clip: u64,
        cut: [i64; 2],
        edges: [Edge; 2],
    },
    Refused {
        #[serde(flatten)]
        reason: Refusal,
    },
}
pub(crate) struct Plan {
    pub outcomes: Vec<Outcome>,
    pub operations: Vec<Operation>,
    pub bindings: BTreeMap<kinewright_core::AssetId, Arc<BoundDialogue>>,
    pub valid: bool,
    pub track: TrackId,
    pub created: Vec<TimeCode>,
    pub changes: super::retention::Changes,
}
struct Cut {
    index: usize,
    track: TrackId,
    clip: ClipId,
    asset: kinewright_core::AssetId,
    start: TimeCode,
    end: TimeCode,
    proposal: [ExactMicros; 2],
    overrule: bool,
    minimum: u64,
}
fn selected(r: &Removal) -> (&WordAddress, &WordAddress, Option<[u64; 2]>, CutRequest) {
    match r {
        Removal::Words {
            first,
            last,
            overrule,
        } => (
            first,
            last,
            *overrule,
            CutRequest::RemoveWords {
                from: first.index,
                to: last.index,
            },
        ),
        Removal::Pause {
            after,
            leave_micros,
            overrule,
        } => (
            after,
            after,
            *overrule,
            CutRequest::TrimPause {
                after_word: after.index,
                leave_micros: *leave_micros,
            },
        ),
    }
}
fn emit(
    doc: &mut Document,
    ops: &mut Vec<Operation>,
    origins: &mut super::retention::Origins,
    op: Operation,
) -> Result<(), Refusal> {
    let split = if let Operation::SplitClip { clip, .. } = &op {
        let next = doc
            .tracks
            .iter()
            .flat_map(|t| &t.clips)
            .map(|c| c.id.0)
            .max()
            .unwrap_or(0)
            .checked_add(1)
            .ok_or_else(|| request("clip id overflow"))?;
        Some((ClipId(next), *origins.get(clip).unwrap_or(clip)))
    } else {
        None
    };
    #[cfg(test)]
    super::cost::operation();
    op.apply_to_unpublished(doc)
        .map_err(|e| request(e.to_string()))?;
    if let Some((child, parent)) = split {
        origins.insert(child, parent);
    }
    if let Operation::DeleteClip { clip } | Operation::RippleDeleteClip { clip } = &op {
        origins.remove(clip);
    }
    ops.push(op);
    Ok(())
}
fn split(
    doc: &mut Document,
    ops: &mut Vec<Operation>,
    origins: &mut super::retention::Origins,
    track: TrackId,
    at: TimeCode,
) -> Result<(), Refusal> {
    let id = doc
        .tracks
        .iter()
        .find(|t| t.id == track)
        .ok_or_else(|| request("track disappeared"))?
        .clips
        .iter()
        .find_map(|c| {
            clip_end(doc, c)
                .ok()
                .filter(|&end| c.timeline_start < at && at < end)
                .map(|_| c.id)
        });
    if let Some(clip) = id {
        emit(doc, ops, origins, Operation::SplitClip { clip, at })?;
    }
    Ok(())
}
fn apply_cut(
    doc: &mut Document,
    ops: &mut Vec<Operation>,
    origins: &mut super::retention::Origins,
    cut: &Cut,
    start: TimeCode,
    end: TimeCode,
) -> Result<(), Refusal> {
    split(doc, ops, origins, cut.track, start)?;
    split(doc, ops, origins, cut.track, end)?;
    let middle = doc
        .tracks
        .iter()
        .find(|t| t.id == cut.track)
        .and_then(|t| {
            t.clips
                .iter()
                .find(|c| c.timeline_start == start && clip_end(doc, c).ok() == Some(end))
        })
        .map(|c| c.id)
        .ok_or_else(|| request("removal must isolate one clip"))?;
    let tracks: Vec<_> = doc
        .tracks
        .iter()
        .filter(|t| t.id != cut.track && t.sync_lock)
        .map(|t| t.id)
        .collect();
    for track in tracks {
        split(doc, ops, origins, track, start)?;
        split(doc, ops, origins, track, end)?;
        let ids: Vec<_> = doc
            .tracks
            .iter()
            .find(|t| t.id == track)
            .into_iter()
            .flat_map(|t| &t.clips)
            .filter(|c| c.timeline_start >= start && clip_end(doc, c).is_ok_and(|e| e <= end))
            .map(|c| c.id)
            .collect();
        for clip in ids {
            emit(doc, ops, origins, Operation::DeleteClip { clip })?;
        }
    }
    emit(
        doc,
        ops,
        origins,
        Operation::RippleDeleteClip { clip: middle },
    )
}
fn project_at(
    doc: &Document,
    clip: &kinewright_core::Clip,
    at: ExactMicros,
    side: AudibleSide,
) -> Result<TimeCode, Refusal> {
    let asset = doc.asset(clip.asset).ok_or(Refusal::Identity)?;
    let origin = ExactMicros::frames(clip.source_range.start, asset.fps)
        .map_err(|e| request(e.to_string()))?;
    let grid = kinewright_core::dialogue::FrameGrid::new(origin, doc.fps)
        .map_err(|e| request(e.to_string()))?;
    let rounding = match side {
        AudibleSide::Out => kinewright_core::FrameRounding::Ceil,
        AudibleSide::In => kinewright_core::FrameRounding::Floor,
    };
    let offset = grid
        .frame_at(at, rounding)
        .map_err(|e| request(e.to_string()))?
        .0;
    clip.timeline_start
        .checked_add(TimeCode(offset))
        .ok_or_else(|| request("timeline overflow"))
}
#[allow(
    clippy::too_many_lines,
    reason = "one authenticated word selection and its two proposed edges"
)]
fn resolve(
    doc: &Document,
    index: usize,
    removal: &Removal,
    value: &BoundDialogue,
) -> Result<Cut, Refusal> {
    let (first, last, overrule, mut req) = selected(removal);
    if first.clip != last.clip || first.binding != last.binding || first.index > last.index {
        return Err(Refusal::Word);
    }
    let clip = doc
        .clip(ClipId(first.clip))
        .ok_or(Refusal::Clip { clip: first.clip })?;
    if clip.speed_percent != 100 {
        return Err(Refusal::Retimed { clip: first.clip });
    }
    let track = doc
        .tracks
        .iter()
        .find(|t| t.clips.iter().any(|c| c.id == clip.id))
        .ok_or(Refusal::Clip { clip: first.clip })?;
    if !doc.track_audible(track.id)
        || !clip.content.is_media()
        || !clip.enabled
        || clip.enabled_curve.is_some()
    {
        return Err(Refusal::Clip { clip: first.clip });
    }
    let asset = doc.asset(clip.asset).ok_or(Refusal::Identity)?;
    let played = spans(doc)?
        .into_iter()
        .find(|s| s.clip == clip.id)
        .ok_or(Refusal::Clip { clip: first.clip })?;
    if played.source.end > ExactMicros::integer(value.dialogue.profile().duration_micros()) {
        return Err(Refusal::NotMeasured);
    }
    let words = value.dialogue.words();
    let visible: Vec<_> = words
        .iter()
        .enumerate()
        .filter(|(_, w)| {
            ExactMicros::integer(w.end_micros) > played.source.start
                && ExactMicros::integer(w.start_micros) < played.source.end
        })
        .map(|(i, _)| i)
        .collect();
    if !visible.contains(&first.index) || !visible.contains(&last.index) {
        return Err(Refusal::Word);
    }
    // The unheard prefix/suffix cannot supply a neighbouring kept word.
    // Resolve it as a recording boundary, then retain the clip's real boundary.
    let (head, tail) = if matches!(removal, Removal::Words { .. }) {
        (
            visible.first() == Some(&first.index),
            visible.last() == Some(&last.index),
        )
    } else {
        (false, false)
    };
    if let CutRequest::RemoveWords { from, to } = &mut req {
        if head {
            *from = 0;
        }
        if tail {
            *to = words.len() - 1;
        }
    }
    let proposal = if let Some(times) = overrule {
        if times[0] >= times[1] {
            return Err(request("overrule needs two forward source instants"));
        }
        [
            ExactMicros::integer(times[0]),
            ExactMicros::integer(times[1]),
        ]
    } else {
        let resolved = value.dialogue.resolve(req).map_err(Refusal::from)?;
        let a = value
            .dialogue
            .frame_edge(resolved.start, asset.fps)
            .map_err(Refusal::from)?;
        let b = value
            .dialogue
            .frame_edge(resolved.end, asset.fps)
            .map_err(Refusal::from)?;
        [
            if head {
                played.source.start
            } else {
                ExactMicros::new(a.instant_numerator, a.instant_denominator)
                    .map_err(|e| request(e.to_string()))?
            },
            if tail {
                played.source.end
            } else {
                ExactMicros::new(b.instant_numerator, b.instant_denominator)
                    .map_err(|e| request(e.to_string()))?
            },
        ]
    };
    if proposal[0] < played.source.start || proposal[1] > played.source.end {
        return Err(request(
            "requested edges are outside the clip's audible span",
        ));
    }
    let start = project_at(doc, clip, proposal[0], AudibleSide::Out)?;
    let end = if tail && overrule.is_none() {
        played.project.end
    } else {
        project_at(doc, clip, proposal[1], AudibleSide::In)?
    };
    let minimum = match removal {
        Removal::Words { .. } => value.dialogue.settings().minimum_word_cut_micros,
        Removal::Pause { .. } => value.dialogue.settings().minimum_pause_cut_micros,
    };
    Ok(Cut {
        index,
        track: track.id,
        clip: clip.id,
        asset: clip.asset,
        start,
        end,
        proposal,
        overrule: overrule.is_some(),
        minimum,
    })
}
fn edge(
    value: &BoundDialogue,
    asset: kinewright_core::AssetId,
    at: ExactMicros,
    frame: TimeCode,
    overrule: bool,
) -> Edge {
    let d = value.dialogue.dip_intrusion_exact(at).map(|d| {
        (
            d.word,
            match d.boundary {
                kinewright_core::dialogue::WordBoundary::Start => "start",
                kinewright_core::dialogue::WordBoundary::End => "end",
            },
            d.micros,
        )
    });
    let placement = if overrule {
        "overrule"
    } else if at == ExactMicros::ZERO
        || at == ExactMicros::integer(value.dialogue.profile().duration_micros())
    {
        "boundary"
    } else if value.dialogue.quiet_runs().iter().any(|r| {
        ExactMicros::integer(r.start_micros()) <= at && at <= ExactMicros::integer(r.end_micros())
    }) {
        "quiet"
    } else {
        "dip"
    };
    Edge(frame.0, asset.0, at, placement, d)
}
type HeardEdges = [Option<(kinewright_core::AssetId, ExactMicros)>; 2];
fn heard_join(
    doc: &Document,
    track: TrackId,
    at: TimeCode,
    values: &mut BTreeMap<kinewright_core::AssetId, Arc<BoundDialogue>>,
    provider: &dyn DialogueMeasurements,
    guard: [bool; 2],
) -> Result<HeardEdges, Refusal> {
    let segments = spans(doc)?;
    let out = segments
        .iter()
        .find(|s| s.track == track && s.project.end == at);
    let inn = segments
        .iter()
        .find(|s| s.track == track && s.project.start == at);
    let mut edges = [None, None];
    for (index, (segment, direction)) in [(out, AudibleSide::Out), (inn, AudibleSide::In)]
        .into_iter()
        .enumerate()
    {
        if let Some(s) = segment {
            if !s.proven {
                return Err(Refusal::UnprovenSegment { clip: s.clip.0 });
            }
            if !values.contains_key(&s.asset) {
                let value = cached(
                    provider,
                    doc.asset(s.asset).ok_or(Refusal::Identity)?,
                    values,
                )?;
                values.insert(s.asset, value);
            }
            let instant = if direction == AudibleSide::Out {
                s.source.end
            } else {
                s.source.start
            };
            if guard[index] {
                values[&s.asset]
                    .dialogue
                    .guard_exact(instant, direction)
                    .map_err(Refusal::from)?;
            }
            edges[index] = Some((s.asset, instant));
        }
    }
    Ok(edges)
}
#[derive(Clone, Copy)]
struct CreatedCut {
    index: usize,
    at: TimeCode,
    requested_end: TimeCode,
    asset: kinewright_core::AssetId,
    proposal: [ExactMicros; 2],
    overrule: bool,
    minimum: u64,
}
fn edge_overrules(created: &[CreatedCut], at: TimeCode) -> [bool; 2] {
    let first = created.iter().find(|c| c.at == at).expect("created join");
    let last = created
        .iter()
        .rev()
        .find(|c| c.at == at)
        .expect("created join");
    // A backed-off right boundary still owns the outgoing edge where its
    // requested end projects. The following removal cannot lend it consent.
    let outgoing = created
        .iter()
        .find(|c| c.requested_end == at)
        .unwrap_or(first);
    [outgoing.overrule, last.overrule]
}
fn keeps(
    edges: HeardEdges,
    asset: kinewright_core::AssetId,
    proposal: [ExactMicros; 2],
    minimum: u64,
) -> Result<(), Refusal> {
    let a = edges[0]
        .filter(|(id, _)| *id == asset)
        .map_or(proposal[0], |(_, t)| t);
    let b = edges[1]
        .filter(|(id, _)| *id == asset)
        .map_or(proposal[1], |(_, t)| t);
    let actual = b.checked_sub(a).map_err(|e| request(e.to_string()))?;
    if actual < ExactMicros::integer(minimum) {
        return Err(Refusal::MinimumCut {
            actual_micros: actual.floor(),
            minimum_micros: minimum,
        });
    }
    if edges[0].is_some_and(|(id, t)| id == asset && t < proposal[0])
        || edges[1].is_some_and(|(id, t)| id == asset && t > proposal[1])
    {
        return Err(request(
            "the final frame mapping would remove extra material",
        ));
    }
    Ok(())
}
struct Trial {
    document: Document,
    operations: Vec<Operation>,
    origins: super::retention::Origins,
    geometry: Vec<CreatedCut>,
    edges: [Edge; 2],
    start: TimeCode,
    end: TimeCode,
}
#[allow(
    clippy::too_many_arguments,
    clippy::too_many_lines,
    reason = "one isolated trial with geometry, lineage, measurements and retained coverage"
)]
fn try_cut(
    doc: &Document,
    cut: &Cut,
    values: &mut BTreeMap<kinewright_core::AssetId, Arc<BoundDialogue>>,
    provider: &dyn DialogueMeasurements,
    removed: i64,
    origins: &super::retention::Origins,
    created: &[CreatedCut],
    retention: &super::retention::Retention,
) -> Result<Trial, Refusal> {
    let mut last = None;
    let mut extent_failure = None;
    for total in 0..=6 {
        for left in (0..=3)
            .rev()
            .filter(|&left| left <= total && total - left <= 3)
        {
            let right = total - left;
            let start = TimeCode(cut.start.0 - removed + left);
            let end = TimeCode(cut.end.0 - removed - right);
            if end.0 - start.0 < 2 {
                // Later, narrower candidates cannot explain why an earlier
                // structurally valid trial failed its measured minimum/guard.
                last.get_or_insert_with(|| {
                    keeps([None, None], cut.asset, cut.proposal, cut.minimum)
                        .err()
                        .unwrap_or_else(|| {
                            request("a dialogue removal needs at least two project frames")
                        })
                });
                continue;
            }
            #[cfg(test)]
            super::cost::trial();
            // The document and tiny lineage overlay belong to this one trial.
            let mut trial = doc.clone();
            let mut next_origins = origins.clone();
            let mut ops = Vec::new();
            if let Err(reason) = apply_cut(&mut trial, &mut ops, &mut next_origins, cut, start, end)
            {
                last = Some(reason);
                continue;
            }
            let width = end.0 - start.0;
            let project = |at: TimeCode| {
                if at >= end {
                    TimeCode(at.0 - width)
                } else if at > start {
                    start
                } else {
                    at
                }
            };
            let mut geometry: Vec<_> = created
                .iter()
                .copied()
                .map(|mut c| {
                    c.at = project(c.at);
                    c.requested_end = project(c.requested_end);
                    c
                })
                .collect();
            geometry.push(CreatedCut {
                index: cut.index,
                at: start,
                requested_end: TimeCode(cut.end.0 - removed - width),
                asset: cut.asset,
                proposal: cut.proposal,
                overrule: cut.overrule,
                minimum: cut.minimum,
            });
            let flags = edge_overrules(&geometry, start);
            let heard = match heard_join(
                &trial,
                cut.track,
                start,
                values,
                provider,
                [!flags[0], !flags[1]],
            ) {
                Ok(h) => h,
                Err(reason) => {
                    last = Some(reason);
                    continue;
                }
            };
            if let Err(reason) = keeps(heard, cut.asset, cut.proposal, cut.minimum) {
                last = Some(reason);
                continue;
            }
            if let Err(reason) = retention.validate(&trial, &next_origins, values, provider) {
                if let Refusal::UnselectedAudio { lost_micros, .. } = &reason {
                    let best = match &extent_failure {
                        Some(Refusal::UnselectedAudio { lost_micros, .. }) => *lost_micros,
                        _ => u64::MAX,
                    };
                    if *lost_micros < best {
                        extent_failure = Some(reason);
                    } else {
                        last = Some(reason);
                    }
                } else {
                    last = Some(reason);
                }
                continue;
            }
            let edges = std::array::from_fn(|i| {
                let (id, t) = heard[i].unwrap_or((cut.asset, cut.proposal[i]));
                edge(
                    &values[&id],
                    id,
                    t,
                    if i == 0 { start } else { end },
                    flags[i],
                )
            });
            return Ok(Trial {
                document: trial,
                operations: ops,
                origins: next_origins,
                geometry,
                edges,
                start,
                end,
            });
        }
    }
    Err(extent_failure.or(last).unwrap_or_else(|| {
        request(
            "no safe keep-side mapping within three project frames; choose wider measured edges",
        )
    }))
}
#[allow(
    clippy::too_many_lines,
    reason = "one atomic plan and final audible geometry"
)]
pub(crate) fn plan(
    doc: &Document,
    args: &CutArgs,
    provider: &dyn DialogueMeasurements,
) -> Result<Plan, Refusal> {
    if args.removals.is_empty() || args.removals.len() > 32 {
        return Err(request("submit 1 to 32 removals"));
    }
    let mut bindings = BTreeMap::new();
    for removal in &args.removals {
        let (first, last, _, _) = selected(removal);
        for address in [first, last] {
            let clip = doc
                .clip(ClipId(address.clip))
                .ok_or(Refusal::Clip { clip: address.clip })?;
            if !bindings.contains_key(&clip.asset) {
                let value = cached(
                    provider,
                    doc.asset(clip.asset).ok_or(Refusal::Identity)?,
                    &bindings,
                )?;
                bindings.insert(clip.asset, value);
            }
            let value = &bindings[&clip.asset];
            if value.binding != address.binding {
                return Err(Refusal::BindingChanged);
            }
            if address.index >= value.dialogue.words().len() {
                return Err(Refusal::Word);
            }
        }
    }
    let mut outcomes = Vec::new();
    let mut cuts = Vec::new();
    for (index, r) in args.removals.iter().enumerate() {
        match resolve(
            doc,
            index,
            r,
            &bindings[&doc
                .clip(ClipId(selected(r).0.clip))
                .ok_or(Refusal::Word)?
                .asset],
        ) {
            Ok(c) => {
                cuts.push(c);
                outcomes.push(None);
            }
            Err(reason) => outcomes.push(Some(Outcome::Refused { reason })),
        }
    }
    let track = cuts.first().map_or(TrackId(0), |c| c.track);
    let windows = cuts
        .iter()
        .map(|c| super::retention::Window {
            clip: c.clip,
            source: c.proposal,
            project: [c.start, c.end],
        })
        .collect();
    let retention = super::retention::Retention::new(doc, track, windows)?;
    cuts.sort_by_key(|c| (c.start, c.end));
    let mut operations = Vec::new();
    let mut working = doc.clone();
    let mut origins = super::retention::Origins::new();
    let mut previous_end = None;
    let mut removed = 0;
    let mut created = Vec::new();
    for cut in cuts {
        let result = if cut.track != track || previous_end.is_some_and(|e| cut.start < e) {
            Err(request("submit disjoint removals on one dialogue track"))
        } else {
            try_cut(
                &working,
                &cut,
                &mut bindings,
                provider,
                removed,
                &origins,
                &created,
                &retention,
            )
        };
        previous_end = Some(cut.end);
        outcomes[cut.index] = Some(match result {
            Ok(Trial {
                document,
                operations: ops,
                origins: next,
                geometry,
                mut edges,
                start,
                end,
            }) => {
                for edge in &mut edges {
                    edge.0 += removed;
                }
                removed += end.0 - start.0;
                working = document;
                origins = next;
                created = geometry;
                operations.extend(ops);
                Outcome::Ready {
                    clip: cut.clip.0,
                    cut: [edges[0].0, edges[1].0],
                    edges,
                }
            }
            Err(reason) => Outcome::Refused { reason },
        });
    }
    for cut in &created {
        let flags = edge_overrules(&created, cut.at);
        let result = heard_join(
            &working,
            track,
            cut.at,
            &mut bindings,
            provider,
            [!flags[0], !flags[1]],
        )
        .and_then(|heard| {
            keeps(heard, cut.asset, cut.proposal, cut.minimum)?;
            Ok(heard)
        });
        match result {
            Ok(heard) => {
                if let Some(Outcome::Ready { edges, .. }) = &mut outcomes[cut.index] {
                    for (i, out) in edges.iter_mut().enumerate() {
                        out.0 = cut.at.0;
                        if let Some((asset, at)) = heard[i] {
                            *out = edge(&bindings[&asset], asset, at, cut.at, flags[i]);
                        }
                    }
                }
            }
            Err(reason) => outcomes[cut.index] = Some(Outcome::Refused { reason }),
        }
    }
    let mut changes = match retention.validate(&working, &origins, &mut bindings, provider) {
        Ok(changes) => changes,
        Err(reason) => {
            if let Some(c) = created.last() {
                outcomes[c.index] = Some(Outcome::Refused { reason });
            }
            super::retention::Changes::default()
        }
    };
    retention.selected_edges(&working, &origins, &mut changes)?;
    let outcomes: Vec<_> = outcomes
        .into_iter()
        .map(|o| o.expect("outcome filled"))
        .collect();
    let valid = outcomes.iter().all(|o| matches!(o, Outcome::Ready { .. }));
    Ok(Plan {
        outcomes,
        operations,
        bindings,
        valid,
        track,
        created: created.into_iter().map(|c| c.at).collect(),
        changes,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use kinewright_core::{
        MediaAsset, MediaKind, MediaSourceFingerprint, Rational, Track, TrackKind,
    };

    struct Provider(Arc<BoundDialogue>);
    impl DialogueMeasurements for Provider {
        fn audio_stream(&self, _: &super::super::MediaIdentity) -> Result<u32, Refusal> {
            Ok(0)
        }
        fn get(
            &self,
            _: &super::super::MediaIdentity,
            _: u32,
        ) -> Result<Arc<BoundDialogue>, Refusal> {
            Ok(Arc::clone(&self.0))
        }
    }

    #[test]
    fn backed_off_sliver_keeps_the_preceding_removals_guard() {
        let mut doc = Document {
            fps: Rational::new(100, 1).unwrap(),
            duration: TimeCode(217),
            ..Document::default()
        };
        doc.media_pool.push(MediaAsset {
            id: kinewright_core::AssetId(1),
            path: "not-opened.wav".into(),
            name: "dialogue".into(),
            duration: TimeCode(400),
            fps: doc.fps,
            kind: MediaKind::Audio,
            resolution: None,
            source_fingerprint: MediaSourceFingerprint {
                content_sha256: Some("a".repeat(64)),
                byte_len: Some(100),
            },
            color_description: kinewright_core::ColorDescription::default(),
            assumed_from: None,
        });
        // Final geometry after A backs its end from 190 to 188, then B removes
        // 190..250: a two-frame sliver survives between the two joins.
        doc.tracks.push(Track{id:TrackId(1),kind:TrackKind::Audio,sync_lock:true,clips:serde_json::from_value(serde_json::json!([
            {"id":1,"asset":1,"source_range":{"start":188,"end":190},"timeline_start":65,"effects":[],"transition_in":null},
            {"id":2,"asset":1,"source_range":{"start":250,"end":400},"timeline_start":67,"effects":[],"transition_in":null}
        ])).unwrap()});
        doc.validate().unwrap();
        let value = Arc::new(
            BoundDialogue::new(
                super::super::MediaIdentity {
                    sha256: "a".repeat(64),
                    byte_length: 100,
                },
                0,
                "sliver".into(),
                kinewright_core::dialogue::SoundProfile::from_hops(
                    10_000,
                    4_000_000,
                    (0..400).map(|i| kinewright_core::dialogue::Hop {
                        rms_dbfs_hundredths: if i == 190 { -1800 } else { -6000 },
                        peak_dbfs_hundredths: if i == 190 { -1800 } else { -6000 },
                        voice: i == 190,
                    }),
                )
                .unwrap(),
                Vec::new(),
                kinewright_core::dialogue::Settings::default(),
            )
            .unwrap(),
        );
        let make = |index, at, requested_end, overrule| CreatedCut {
            index,
            at: TimeCode(at),
            requested_end: TimeCode(requested_end),
            asset: kinewright_core::AssetId(1),
            proposal: [
                ExactMicros::integer(650_000),
                ExactMicros::integer(1_900_000),
            ],
            overrule,
            minimum: 100_000,
        };
        let geometry = [make(0, 65, 67, false), make(1, 67, 67, true)];
        let flags = edge_overrules(&geometry, TimeCode(67));
        let provider = Provider(Arc::clone(&value));
        let result = heard_join(
            &doc,
            TrackId(1),
            TimeCode(67),
            &mut BTreeMap::from([(kinewright_core::AssetId(1), value)]),
            &provider,
            [!flags[0], !flags[1]],
        );
        assert!(
            matches!(
                result,
                Err(Refusal::Acoustic {
                    at_us: 1_900_000,
                    ..
                })
            ),
            "DQ_TOOLS_SCOPED_OVERRULE: {result:?}"
        );
    }
}
