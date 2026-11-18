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
    /// Require every member; by default the ready subset applies as one edit.
    #[serde(default)]
    pub atomic: bool,
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
        leave_micros: Option<u64>,
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
#[allow(
    clippy::large_enum_variant,
    reason = "at most 32 inline wire outcomes; boxing each successful edge pair adds an allocation"
)]
pub(crate) enum Outcome {
    Ready {
        index: usize,
        clip: u64,
        cut: [i64; 2],
        edges: [Edge; 2],
        #[serde(skip_serializing_if = "Option::is_none")]
        kept_air_micros: Option<ExactMicros>,
        #[serde(skip_serializing_if = "Vec::is_empty")]
        other_pauses_micros: Vec<u64>,
    },
    Refused {
        index: usize,
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
#[derive(Clone, Copy)]
struct AddressSpan {
    clip: ClipId,
    source: [ExactMicros; 2],
}
struct Cut {
    addresses: [AddressSpan; 2],
    request: CutRequest,
    index: usize,
    track: TrackId,
    clip: ClipId,
    asset: kinewright_core::AssetId,
    start: TimeCode,
    end: TimeCode,
    proposal: [ExactMicros; 2],
    overrule: bool,
    minimum: u64,
    air: Option<kinewright_core::dialogue::RetainedAir>,
    other_pauses_micros: Vec<u64>,
}
fn selected(r: &Removal) -> (&WordAddress, &WordAddress, Option<[u64; 2]>, CutRequest) {
    match r {
        Removal::Words {
            first,
            last,
            leave_micros,
            overrule,
        } => (
            first,
            last,
            *overrule,
            CutRequest::RemoveWords {
                from: first.index,
                to: last.index,
                leave_micros: *leave_micros,
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
    let evidence_request = req;
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
    if let CutRequest::RemoveWords { from, to, .. } = &mut req {
        if head {
            *from = 0;
        }
        if tail {
            *to = words.len() - 1;
        }
    }
    let mut air = None;
    let mut other_pauses_micros = Vec::new();
    let proposal = if let Some(times) = overrule {
        if times[0] >= times[1] {
            return Err(request("overrule needs two forward source instants"));
        }
        [
            ExactMicros::integer(times[0]),
            ExactMicros::integer(times[1]),
        ]
    } else {
        let resolved = match req {
            CutRequest::TrimPause {
                after_word,
                leave_micros,
            } => {
                let available = kinewright_core::dialogue::Span::new(
                    played.source.start.ceil(),
                    played.source.end.floor(),
                )
                .map_err(|e| request(e.to_string()))?;
                value
                    .dialogue
                    .resolve_pause_in(after_word, leave_micros, available)
            }
            CutRequest::RemoveWords { .. } => value.dialogue.resolve(req),
        }
        .map_err(Refusal::from)?;
        air = resolved.air;
        other_pauses_micros = resolved.other_pauses_micros;
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
    value
        .dialogue
        .validate_cut(evidence_request, proposal)
        .map_err(Refusal::from)?;
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
        request: evidence_request,
        addresses: [first, last].map(|a| {
            let word = &words[a.index];
            AddressSpan {
                clip: ClipId(a.clip),
                source: [
                    ExactMicros::integer(word.start_micros).max(played.source.start),
                    ExactMicros::integer(word.end_micros).min(played.source.end),
                ],
            }
        }),
        index,
        track: track.id,
        clip: clip.id,
        asset: clip.asset,
        start,
        end,
        proposal,
        overrule: overrule.is_some(),
        minimum,
        air,
        other_pauses_micros,
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
    request: CutRequest,
    original_start: TimeCode,
    air: Option<[bool; 2]>,
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
) -> Result<[ExactMicros; 2], Refusal> {
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
    Ok([a, b])
}
fn verify_selection(
    values: &BTreeMap<kinewright_core::AssetId, Arc<BoundDialogue>>,
    removed: &CreatedCut,
    heard: HeardEdges,
) -> Result<(), Refusal> {
    let extent = keeps(heard, removed.asset, removed.proposal, removed.minimum)?;
    values[&removed.asset]
        .dialogue
        .validate_cut(removed.request, extent)
        .map_err(Refusal::from)
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
                request: cut.request,
                original_start: cut.start,
                air: cut.air.map(|a| [a.left.is_some(), a.right.is_some()]),
                index: cut.index,
                at: start,
                requested_end: TimeCode(cut.end.0 - removed - width),
                asset: cut.asset,
                proposal: cut.proposal,
                overrule: cut.overrule,
                minimum: cut.minimum,
            });
            geometry.sort_by_key(|c| c.original_start);
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
            let current = geometry
                .iter()
                .find(|c| c.index == cut.index)
                .expect("current removal");
            if let Err(reason) = verify_selection(values, current, heard) {
                last = Some(reason);
                continue;
            }
            let prior = geometry
                .iter()
                .filter(|c| c.index != cut.index)
                .try_for_each(|c| {
                    let flags = edge_overrules(&geometry, c.at);
                    let heard = heard_join(
                        &trial,
                        cut.track,
                        c.at,
                        values,
                        provider,
                        [!flags[0], !flags[1]],
                    )?;
                    verify_selection(values, c, heard)
                });
            if let Err(reason) = prior {
                last = Some(reason);
                continue;
            }
            if let Err(reason) = retention.validate(&trial, &next_origins, values, provider) {
                if let Some(changed_micros) = reason.extent_change_micros() {
                    let best = extent_failure
                        .as_ref()
                        .and_then(Refusal::extent_change_micros)
                        .unwrap_or(u64::MAX);
                    if changed_micros < best {
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

fn air_side(
    pieces: &[super::check::PlayedSpan],
    track: TrackId,
    at: TimeCode,
    outgoing: bool,
    values: &BTreeMap<kinewright_core::AssetId, Arc<BoundDialogue>>,
    used: &mut std::collections::BTreeSet<kinewright_core::AssetId>,
) -> Option<ExactMicros> {
    let mut total = ExactMicros::ZERO;
    let mut cursor = at;
    let mut nearby = pieces.iter().filter(|s| {
        s.track == track
            && if outgoing {
                s.project.end <= at
            } else {
                s.project.start >= at
            }
    });
    while let Some(span) = if outgoing {
        nearby.next_back()
    } else {
        nearby.next()
    } {
        let boundary = if outgoing {
            span.project.end
        } else {
            span.project.start
        };
        if boundary != cursor
            || !span.proven
            || outgoing && span.source.padding != ExactMicros::ZERO
        {
            return None;
        }
        let value = values.get(&span.asset)?;
        used.insert(span.asset);
        let edge = if outgoing {
            span.source.end
        } else {
            span.source.start
        };
        let run = value.dialogue.gap_runs().iter().find(|run| {
            let start = ExactMicros::integer(run.start_micros());
            let end = ExactMicros::integer(run.end_micros());
            if outgoing {
                start < edge && edge <= end
            } else {
                start <= edge && edge < end
            }
        });
        let Some(run) = run else {
            return Some(total);
        };
        let lo = ExactMicros::integer(run.start_micros()).max(span.source.start);
        let hi = ExactMicros::integer(run.end_micros()).min(span.source.end);
        total = total.checked_add(hi.checked_sub(lo).ok()?).ok()?;
        if (outgoing && lo > span.source.start) || (!outgoing && hi < span.source.end) {
            return Some(total);
        }
        if span.source.padding != ExactMicros::ZERO {
            return None;
        }
        cursor = if outgoing {
            span.project.start
        } else {
            span.project.end
        };
    }
    Some(total)
}
fn retained_air(
    mask: Option<[bool; 2]>,
    pieces: &[super::check::PlayedSpan],
    track: TrackId,
    at: TimeCode,
    values: &BTreeMap<kinewright_core::AssetId, Arc<BoundDialogue>>,
    used: &mut std::collections::BTreeSet<kinewright_core::AssetId>,
) -> Option<ExactMicros> {
    let mask = mask?;
    let left = if mask[0] {
        air_side(pieces, track, at, true, values, used)?
    } else {
        ExactMicros::ZERO
    };
    let right = if mask[1] {
        air_side(pieces, track, at, false, values, used)?
    } else {
        ExactMicros::ZERO
    };
    left.checked_add(right).ok()
}

struct Accepted {
    asset: kinewright_core::AssetId,
    index: usize,
    window: super::retention::Window,
    addresses: [AddressSpan; 2],
    width: i64,
}
fn removes(window: super::retention::Window, address: AddressSpan) -> bool {
    window.clip == address.clip
        && window.source[0] <= address.source[0]
        && window.source[1] >= address.source[1]
}
fn conflict(cut: &Cut, accepted: &[Accepted]) -> Option<Refusal> {
    let window = super::retention::Window {
        clip: cut.clip,
        source: cut.proposal,
        project: [cut.start, cut.end],
    };
    accepted.iter().find_map(|old| {
        let cause = if window.project[0] < old.window.project[1]
            && old.window.project[0] < window.project[1]
        {
            super::ConflictCause::Overlap
        } else if old.addresses.iter().any(|&a| removes(window, a))
            || cut.addresses.iter().any(|&a| removes(old.window, a))
        {
            super::ConflictCause::AddressRemoved
        } else {
            return None;
        };
        Some(Refusal::Conflict {
            other_index: old.index,
            cause,
        })
    })
}
#[allow(
    clippy::too_many_lines,
    reason = "one snapshot plan with independently admitted members"
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
    // Every address and proposal is resolved on the original immutable revision.
    let proposals: Vec<_> = args
        .removals
        .iter()
        .enumerate()
        .map(|(index, removal)| {
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
            resolve(
                doc,
                index,
                removal,
                &bindings[&doc.clip(ClipId(first.clip)).ok_or(Refusal::Word)?.asset],
            )
        })
        .collect();
    let mut accepted: Vec<Accepted> = Vec::new();
    let mut outcomes = Vec::new();
    let mut working = doc.clone();
    let mut operations = Vec::new();
    let mut origins = super::retention::Origins::new();
    let mut created = Vec::new();
    let mut track = None;
    for (index, proposal) in proposals.into_iter().enumerate() {
        let cut = match proposal {
            Ok(c) => c,
            Err(reason) => {
                outcomes.push(Outcome::Refused { index, reason });
                continue;
            }
        };
        if track.is_some_and(|t| t != cut.track) {
            outcomes.push(Outcome::Refused {
                index,
                reason: request("submit removals on one dialogue track"),
            });
            continue;
        }
        if let Some(reason) = conflict(&cut, &accepted) {
            outcomes.push(Outcome::Refused { index, reason });
            continue;
        }
        let window = super::retention::Window {
            clip: cut.clip,
            source: cut.proposal,
            project: [cut.start, cut.end],
        };
        // Refused proposals never exempt source coverage from the extent checks.
        let windows = accepted
            .iter()
            .map(|c| c.window)
            .chain(std::iter::once(window))
            .collect();
        let retention = super::retention::Retention::new(doc, cut.track, windows)?;
        let removed = accepted
            .iter()
            .filter(|c| c.window.project[0] < cut.start)
            .map(|c| c.width)
            .sum();
        let result = try_cut(
            &working,
            &cut,
            &mut bindings,
            provider,
            removed,
            &origins,
            &created,
            &retention,
        );
        match result {
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
                accepted.push(Accepted {
                    asset: cut.asset,
                    index,
                    window,
                    addresses: cut.addresses,
                    width: end.0 - start.0,
                });
                track = Some(cut.track);
                working = document;
                origins = next;
                created = geometry;
                operations.extend(ops);
                outcomes.push(Outcome::Ready {
                    index,
                    clip: cut.clip.0,
                    cut: [edges[0].0, edges[1].0],
                    edges,
                    kept_air_micros: None,
                    other_pauses_micros: cut.other_pauses_micros,
                });
            }
            Err(reason) => outcomes.push(Outcome::Refused { index, reason }),
        }
    }
    let track = track.unwrap_or(TrackId(0));
    let mut final_spans = spans(&working)?;
    final_spans.sort_by_key(|s| (s.track, s.project.start));
    let mut air_assets = std::collections::BTreeSet::new();
    for c in &created {
        let flags = edge_overrules(&created, c.at);
        let heard = heard_join(
            &working,
            track,
            c.at,
            &mut bindings,
            provider,
            [!flags[0], !flags[1]],
        )?;
        verify_selection(&bindings, c, heard)?;
        if let Outcome::Ready {
            edges,
            kept_air_micros,
            ..
        } = &mut outcomes[c.index]
        {
            *kept_air_micros =
                retained_air(c.air, &final_spans, track, c.at, &bindings, &mut air_assets);
            for (i, out) in edges.iter_mut().enumerate() {
                out.0 = c.at.0;
                if let Some((asset, at)) = heard[i] {
                    *out = edge(&bindings[&asset], asset, at, c.at, flags[i]);
                }
            }
        }
    }
    let retention =
        super::retention::Retention::new(doc, track, accepted.iter().map(|c| c.window).collect())?;
    let mut changes = retention.validate(&working, &origins, &mut bindings, provider)?;
    // Refused members are not commit dependencies. Keep only addressed and
    // newly audible measurements actually used to authorize the ready edit.
    let mut proof: std::collections::BTreeSet<_> =
        accepted.iter().map(|c| c.asset).chain(air_assets).collect();
    for span in final_spans {
        let edge = |at| {
            (span.track == track && created.iter().any(|c| c.at == at))
                || changes
                    .positions
                    .get(&span.track)
                    .is_some_and(|p| p.contains(&at))
        };
        if edge(span.project.start) || edge(span.project.end) {
            proof.insert(span.asset);
        }
    }
    bindings.retain(|id, _| proof.contains(id));
    // Unchanged neighbours added for readback are not authorization inputs.
    retention.selected_edges(&working, &origins, &mut changes)?;
    let valid = !created.is_empty()
        && (!args.atomic || outcomes.iter().all(|o| matches!(o, Outcome::Ready { .. })));
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
            remote_locator: None,
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
        doc.tracks.push(Track{ name: None,id:TrackId(1),kind:TrackKind::Audio,sync_lock:true,clips:serde_json::from_value(serde_json::json!([
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
            request: CutRequest::RemoveWords {
                from: 0,
                to: 0,
                leave_micros: None,
            },
            original_start: TimeCode(at),
            air: None,
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
