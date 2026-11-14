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
    internal: [bool; 2],
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
fn emit(doc: &mut Document, ops: &mut Vec<Operation>, op: Operation) -> Result<(), Refusal> {
    op.apply(doc).map_err(|e| request(e.to_string()))?;
    ops.push(op);
    Ok(())
}
fn split(
    doc: &mut Document,
    ops: &mut Vec<Operation>,
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
        emit(doc, ops, Operation::SplitClip { clip, at })?;
    }
    Ok(())
}
fn apply_cut(
    doc: &mut Document,
    ops: &mut Vec<Operation>,
    cut: &Cut,
    start: TimeCode,
    end: TimeCode,
) -> Result<(), Refusal> {
    split(doc, ops, cut.track, start)?;
    split(doc, ops, cut.track, end)?;
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
    // Match transcript deletion: isolate and delete other sync-locked portions,
    // then ripple exactly once on the selected dialogue clip.
    let tracks: Vec<_> = doc
        .tracks
        .iter()
        .filter(|t| t.id != cut.track && t.sync_lock)
        .map(|t| t.id)
        .collect();
    for track in tracks {
        split(doc, ops, track, start)?;
        split(doc, ops, track, end)?;
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
            emit(doc, ops, Operation::DeleteClip { clip })?;
        }
    }
    emit(doc, ops, Operation::RippleDeleteClip { clip: middle })
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
        internal: [false, false],
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
struct CreatedCut {
    index: usize,
    at: TimeCode,
    asset: kinewright_core::AssetId,
    proposal: [ExactMicros; 2],
    overrule: bool,
    minimum: u64,
}
fn keeps(
    edges: HeardEdges,
    asset: kinewright_core::AssetId,
    proposal: [ExactMicros; 2],
    minimum: u64,
) -> bool {
    let a = edges[0]
        .filter(|(id, _)| *id == asset)
        .map_or(proposal[0], |(_, t)| t);
    let b = edges[1]
        .filter(|(id, _)| *id == asset)
        .map_or(proposal[1], |(_, t)| t);
    if !b
        .checked_sub(a)
        .is_ok_and(|d| d >= ExactMicros::integer(minimum))
    {
        return false;
    }

    !edges[0].is_some_and(|(id, t)| id == asset && t < proposal[0])
        && !edges[1].is_some_and(|(id, t)| id == asset && t > proposal[1])
}
struct Trial {
    document: Document,
    operations: Vec<Operation>,
    edges: [Edge; 2],
    start: TimeCode,
    end: TimeCode,
}
fn try_cut(
    doc: &Document,
    cut: &Cut,
    values: &mut BTreeMap<kinewright_core::AssetId, Arc<BoundDialogue>>,
    provider: &dyn DialogueMeasurements,
    removed: i64,
) -> Result<Trial, Refusal> {
    let asset = cut.asset;
    let mut last = None;
    // At most three frames per side, always keeping more material. Each trial
    // owns a speculative document because real SplitClip chooses source aliases.
    for left in 0..=3 {
        for right in 0..=3 {
            let start = TimeCode(cut.start.0 - removed + left);
            let end = TimeCode(cut.end.0 - removed - right);
            if end.0 - start.0 < 2 {
                continue;
            }
            let duration = ExactMicros::frames(TimeCode(end.0 - start.0), doc.fps)
                .map_err(|e| request(e.to_string()))?;
            if duration < ExactMicros::integer(cut.minimum) {
                continue;
            }
            let mut trial = doc.clone();
            let mut ops = Vec::new();
            apply_cut(&mut trial, &mut ops, cut, start, end)?;
            let heard = match heard_join(
                &trial,
                cut.track,
                start,
                values,
                provider,
                [
                    !cut.overrule && !cut.internal[0],
                    !cut.overrule && !cut.internal[1],
                ],
            ) {
                Ok(h) => h,
                Err(reason) => {
                    last = Some(reason);
                    continue;
                }
            };
            if !keeps(heard, asset, cut.proposal, cut.minimum) {
                continue;
            }
            let edges = std::array::from_fn(|i| {
                let (id, t) = heard[i].unwrap_or((asset, cut.proposal[i]));
                edge(
                    &values[&id],
                    id,
                    t,
                    if i == 0 { start } else { end },
                    cut.overrule,
                )
            });
            return Ok(Trial {
                document: trial,
                operations: ops,
                edges,
                start,
                end,
            });
        }
    }
    Err(last.unwrap_or_else(|| {
        request(
            "no safe keep-side mapping within three project frames; choose wider measured edges",
        )
    }))
}
#[allow(
    clippy::too_many_lines,
    reason = "all-or-nothing batch and final audible-edge validation"
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
    // Finish identity/index authentication before resolving any removal.
    for removal in &args.removals {
        let (first, last, _, _) = selected(removal);
        for address in [first, last] {
            let id = ClipId(address.clip);
            let clip = doc.clip(id).ok_or(Refusal::Clip { clip: address.clip })?;
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
    // Timeline order preserves the source in-points of previously guarded
    // incoming pieces. Coordinates below subtract the accepted ripple prefix.
    cuts.sort_by_key(|c| (c.start, c.end));
    // Shared inner boundaries disappear; the surviving outer sides own guards.
    for i in 0..cuts.len() {
        let out = i > 0 && cuts[i - 1].end == cuts[i].start;
        let inn = i + 1 < cuts.len() && cuts[i].end == cuts[i + 1].start;
        cuts[i].internal = [out, inn];
    }
    let mut operations = Vec::new();
    let mut working = doc.clone();
    let mut previous_end = None;
    let mut removed = 0;
    let mut created: Vec<CreatedCut> = Vec::new();
    for cut in cuts {
        let result = if cut.track != track || previous_end.is_some_and(|e| cut.start < e) {
            Err(request("submit disjoint removals on one dialogue track"))
        } else {
            try_cut(&working, &cut, &mut bindings, provider, removed)
        };
        previous_end = Some(cut.end);
        outcomes[cut.index] = Some(match result {
            Ok(Trial {
                document: trial,
                operations: ops,
                mut edges,
                start,
                end,
            }) => {
                for edge in &mut edges {
                    edge.0 += removed;
                }
                removed += end.0 - start.0;
                created.push(CreatedCut {
                    index: cut.index,
                    at: start,
                    asset: cut.asset,
                    proposal: cut.proposal,
                    overrule: cut.overrule,
                    minimum: cut.minimum,
                });
                working = trial;
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
    // A later split can re-anchor an earlier outgoing portion at mixed rates.
    // Validate every created edge in the final speculative document as well.
    // Frame backoffs can separate cuts, so determine ownership at final joins.
    for cut in &created {
        let mut owners = created.iter().filter(|owner| owner.at == cut.at);
        let first = owners.next().expect("current cut");
        let last = owners.next_back().unwrap_or(first);
        let overrule = [first.overrule, last.overrule];
        match heard_join(
            &working,
            track,
            cut.at,
            &mut bindings,
            provider,
            [!overrule[0], !overrule[1]],
        ) {
            Ok(heard) if keeps(heard, cut.asset, cut.proposal, cut.minimum) => {
                if let Some(Outcome::Ready { edges, .. }) = &mut outcomes[cut.index] {
                    for (i, edge_result) in edges.iter_mut().enumerate() {
                        edge_result.0 = cut.at.0;
                        if let Some((asset, at)) = heard[i] {
                            *edge_result = edge(
                                &bindings[&asset],
                                asset,
                                at,
                                TimeCode(edge_result.0),
                                overrule[i],
                            );
                        }
                    }
                }
            }
            Ok(_) => {
                outcomes[cut.index] = Some(Outcome::Refused {
                    reason: request("the final frame mapping would remove extra material"),
                });
            }
            Err(reason) => outcomes[cut.index] = Some(Outcome::Refused { reason }),
        }
    }
    let outcomes: Vec<_> = outcomes
        .into_iter()
        .map(|o| o.expect("every outcome filled"))
        .collect();
    let valid = outcomes.iter().all(|o| matches!(o, Outcome::Ready { .. }));
    Ok(Plan {
        outcomes,
        operations,
        bindings,
        valid,
        track,
        created: created.into_iter().map(|c| c.at).collect(),
    })
}
