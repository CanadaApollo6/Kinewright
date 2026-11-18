mod clips;
mod grid;
mod layout;
mod nest_transitions;
mod transition;

use std::{
    collections::{BTreeMap, BTreeSet},
    ops::Range,
};

use kinewright_core::{
    ClipContent, ClipId, Document, Fidelity, ImportRecord, InterchangeFormat, LossBound, LossCode,
    Marker, Rational, TRACK_NAME_MAX_BYTES, TimeCode, Title, TrackKind,
};

use crate::{
    MAX_ITEMS, MAX_NEST_DEPTH, MAX_TRACKS_PER_KIND, Refusal, RefusalCode, Time, check_rate,
    ir::{self, ContainerTime, Item, Loss, Placed, Stack, Timeline},
    media::{Assets, MediaTable, valid_resolution},
    parse,
    record::{Report, SitePage, SiteQuery},
    shorten,
};

use self::{
    clips::{Candidate, ClipInput},
    layout::{Block, Lane, Pair, Stored, too_many_tracks},
    transition::{TransitionInput, lower_same_track_transition},
};

mod observations;
mod output;

#[derive(Debug)]
pub struct Lowered {
    pub document: Document,
    pub record: ImportRecord,
    pub sites: SitePage,
}

/// Lower a timeline using media results collected by the caller.
///
/// # Errors
/// Refuses over-cap, arithmetically unrepresentable or empty imports.
pub fn lower(timeline: &Timeline, media: &MediaTable) -> Result<(Document, ImportRecord), Refusal> {
    let lowered = lower_with_sites(timeline, media, None)?;
    Ok((lowered.document, lowered.record))
}

/// Lower a timeline and optionally collect a page of source sites.
///
/// # Errors
/// Refuses over-cap, arithmetically unrepresentable or empty imports.
pub fn lower_with_sites(
    timeline: &Timeline,
    media: &MediaTable,
    query: Option<SiteQuery>,
) -> Result<Lowered, Refusal> {
    let fps = check_rate(timeline.rate_hint)?;
    let assets = Assets::build(timeline, media)?;
    let (resolution, inferred) = project_resolution(timeline, &assets);
    let mut state = State {
        report: Report::new(query),
        project: fps,
        format: timeline.source.format,
        assets,
        next_clip: 0,
        markers: Vec::new(),
        gaps: Vec::new(),
        item_limit: 0,
    };
    let start_offset_frames = timeline.start.frame(fps)?.0;
    state.initial_report(timeline, start_offset_frames, inferred);
    let mut block = state.stack(&timeline.root, timeline.root.time, &(0..i64::MAX), 0)?;
    if timeline.source.format == InterchangeFormat::Otio {
        state.collapse_dual_mono(&mut block.audio);
    }
    let (tracks, duration) = state.output_tracks(block)?;
    if state.report.is_empty() && state.gaps.is_empty() {
        return Err(Refusal::new(
            RefusalCode::NothingToImport,
            "The selected timeline contains no importable items.",
        ));
    }
    state.finish_gaps(duration.0);
    let mut record = ImportRecord {
        format: timeline.source.format,
        format_version: timeline.source.version.clone(),
        source_name: timeline.source.name.clone(),
        source_sha256: timeline.source.sha256.clone(),
        start_offset_frames,
        items: 0,
        exact: 0,
        approximated: 0,
        dropped: 0,
        media: state.assets.counts,
        estimated_assets: state.assets.estimated,
        retimed_source_end_drift_frames: None,
        rows: Vec::new(),
        omitted_codes: Vec::new(),
    };
    let sites = state.report.finish(&mut record)?;
    let document = Document {
        tracks,
        media_pool: state.assets.pool,
        markers: state.markers,
        fps,
        resolution,
        duration,
        imported_from: vec![record.clone()],
        ..Document::default()
    };
    document.validate().map_err(|error| {
        Refusal::new(
            RefusalCode::LoweringFailed,
            format!("The lowered timeline is invalid: {error}."),
        )
    })?;
    Ok(Lowered {
        document,
        record,
        sites,
    })
}

fn project_resolution(timeline: &Timeline, assets: &Assets) -> ((u32, u32), bool) {
    if let Some(resolution) = valid_resolution(timeline.resolution_hint) {
        return (resolution, false);
    }
    let mut counts = BTreeMap::<_, usize>::new();
    for asset in &assets.pool {
        if let Some(size) = asset.resolution {
            *counts.entry(size).or_default() += 1;
        }
    }
    (
        counts
            .into_iter()
            .max_by_key(|&(size, count)| (count, size))
            .map_or((1_920, 1_080), |(size, _)| size),
        true,
    )
}

struct State {
    report: Report,
    project: Rational,
    format: InterchangeFormat,
    assets: Assets,
    next_clip: u64,
    markers: Vec<Marker>,
    gaps: Vec<(String, i64, i64)>,
    item_limit: usize,
}

#[derive(Clone, Copy)]
struct Location<'a> {
    placed: &'a Placed,
    start: Time,
    end: Time,
}

struct Slot<'a> {
    location: Location<'a>,
    candidate: Option<Candidate>,
    pair: Option<Pair>,
}

struct Overlay {
    nests: Vec<(String, i64, Block)>,
    anchors: BTreeMap<i64, Block>,
}

#[derive(Default)]
struct StackTransitions<'a> {
    applied_video: BTreeSet<&'a str>,
    otio: nest_transitions::SharedCuts<'a>,
}

impl State {
    fn id(&mut self) -> ClipId {
        self.next_clip += 1;
        ClipId(self.next_clip)
    }

    fn stack(
        &mut self,
        stack: &Stack,
        map: ContainerTime,
        window: &Range<i64>,
        depth: usize,
    ) -> Result<Block, Refusal> {
        let mut block = Block::default();
        self.decorations(&stack.decorations, map, None)?;
        let mut transitions = StackTransitions::default();
        if self.format == InterchangeFormat::Otio {
            transitions.otio = nest_transitions::SharedCuts::for_stack(stack)?;
        }
        for track in &stack.tracks {
            block.append(self.track(track, map, window, depth, &mut transitions)?)?;
        }
        for (at, cuts) in transitions.otio.frames {
            let frame = map.to_parent(at)?.frame(self.project)?.0;
            for (site, frames) in cuts {
                self.report.loss(
                    &Loss::approximation(LossCode::TransitionAtNestCut)
                        .bounded(LossBound::Frames(frames)),
                    site,
                    frame,
                    None,
                );
            }
        }
        Ok(block)
    }

    fn track<'a>(
        &mut self,
        track: &'a ir::Track,
        map: ContainerTime,
        window: &Range<i64>,
        depth: usize,
        transitions: &mut StackTransitions<'a>,
    ) -> Result<Block, Refusal> {
        if depth > MAX_NEST_DEPTH {
            return Err(parse::too_deep());
        }
        self.decorations(&track.decorations, map, None)?;
        let name = track.name.as_deref().map(|name| {
            let short = shorten(name, TRACK_NAME_MAX_BYTES);
            if short.len() != name.len() {
                self.report.loss(
                    &Loss::approximation(LossCode::TrackNamesDropped).named(name),
                    "",
                    0,
                    None,
                );
            }
            short
        });
        let mut locations = Vec::with_capacity(track.items.len());
        for placed in &track.items {
            self.item_limit += 1;
            if self.item_limit > MAX_ITEMS {
                return Err(parse::too_many_items());
            }
            let start = map.to_parent(placed.at)?;
            if let Some(count) = placed.item.zero_duration_count() {
                if placed.counts_item {
                    let frame = start.frame(self.project)?.0;
                    self.report.item(&placed.site, count);
                    self.report.loss_many(
                        &Loss::dropped(LossCode::ClipTooShort),
                        &placed.site,
                        frame,
                        None,
                        count,
                    );
                }
                continue;
            }
            locations.push(Location {
                start,
                end: map.to_parent(placed.at.checked_add(placed.item.span())?)?,
                placed,
            });
        }
        locations.sort_by_key(|location| location.start);
        let mut result = Block::default();
        let mut overlay = Overlay {
            nests: Vec::new(),
            anchors: BTreeMap::new(),
        };
        for (lane, locations) in distribute(locations)?.iter().enumerate() {
            let name = lane_name(name, lane);
            let sub = self.track_lane(
                track.kind,
                name.as_deref(),
                locations,
                map,
                window,
                depth,
                &mut overlay,
                lane != 0,
                transitions,
            )?;
            result.append(sub)?;
        }
        self.merge_overlay(result, overlay)
    }

    fn merge_overlay(&mut self, mut base: Block, overlay: Overlay) -> Result<Block, Refusal> {
        for (site, frame, nest) in overlay.nests {
            if base.overlay(nest)? {
                self.report.loss(
                    &Loss::approximation(LossCode::LaneAdded),
                    &site,
                    frame,
                    None,
                );
            }
        }
        let mut below = Block::default();
        let mut above = Block::default();
        for (lane, mut anchor) in overlay.anchors {
            if lane < 0 {
                let audio = std::mem::take(&mut anchor.audio);
                below.append(anchor)?;
                // Spine audio comes before audio on a connected lane.
                above.append(Block {
                    video: Vec::new(),
                    audio,
                })?;
            } else {
                above.append(anchor)?;
            }
        }
        below.append(base)?;
        below.append(above)?;
        Ok(below)
    }

    #[allow(
        clippy::too_many_arguments,
        clippy::too_many_lines,
        reason = "Lowering a lane needs its parent transform and reserved blocks"
    )]
    fn track_lane<'a>(
        &mut self,
        kind: TrackKind,
        name: Option<&str>,
        locations: &[Location<'a>],
        map: ContainerTime,
        window: &Range<i64>,
        depth: usize,
        overlay: &mut Overlay,
        spill: bool,
        transitions: &mut StackTransitions<'a>,
    ) -> Result<Block, Refusal> {
        let mut slots = Vec::with_capacity(locations.len());
        let mut previous = None;
        let mut block = Block::default();
        block.lanes(kind).push(Lane {
            name: name.map(str::to_owned),
            clips: BTreeMap::new(),
        });
        let mut next_positive = locations
            .iter()
            .enumerate()
            .filter(|(_, location)| location.end > location.start)
            .peekable();
        for (index, location) in locations.iter().copied().enumerate() {
            if next_positive.peek().is_some_and(|(next, _)| *next == index) {
                next_positive.next();
            }
            let placed = location.placed;
            let at = location.start.frame(self.project)?.0;
            let end = location.end.frame(self.project)?.0;
            let next = next_positive.peek().map(|(_, location)| *location);
            let limit = self.limit(location, next, window)?;
            let start = previous
                .filter(|(raw_end, _)| *raw_end == location.start)
                .map_or(at, |(_, decided)| decided)
                .max(window.start);
            let mut notes = Vec::new();
            let mut candidate = None;
            if placed.counts_item && spill {
                notes.push(Loss::approximation(LossCode::LaneAdded));
            }
            match &placed.item {
                Item::Discarded { count, loss } => {
                    self.report.item(&placed.site, *count);
                    self.report.loss_many(loss, &placed.site, at, None, *count);
                }
                Item::Transition { .. } => {
                    if placed.counts_item {
                        self.report.item(&placed.site, 1);
                    }
                }
                Item::Gap { .. }
                    if placed.notes.iter().all(|note| !note.code.is_item())
                        && location.end > location.start =>
                {
                    if placed.counts_item {
                        self.gaps.push((placed.site.clone(), at, end));
                    }
                    previous = None;
                }
                Item::Nested { stack, .. } => {
                    let frame_end = end.min(window.end).max(start);
                    if let Some(nest) =
                        self.nested(location, stack, map, &(start..frame_end), depth, &mut notes)?
                    {
                        overlay.nests.push((placed.site.clone(), start, nest));
                    }
                    previous = Some((location.end, frame_end));
                }
                _ => {
                    if placed.counts_item {
                        self.report.item(&placed.site, 1);
                    }
                    candidate = self.item(location, kind, map, start, limit, &mut notes)?;
                    previous = candidate
                        .as_ref()
                        .map(|candidate| (location.end, candidate.end));
                    if candidate.is_none()
                        && notes.iter().any(|note| note.code == LossCode::ClipTooShort)
                    {
                        previous = Some((location.end, start));
                    }
                }
            }
            let id = candidate.as_ref().map(|candidate| candidate.clip.id);
            if !matches!(placed.item, Item::Transition { .. }) {
                self.losses(placed.notes.iter().chain(&notes), &placed.site, start, id);
            }
            self.item_decorations(
                location,
                map,
                start,
                candidate.as_ref().map_or(end, |clip| clip.end),
            )?;
            self.anchor_blocks(placed, map, window, depth, &mut overlay.anchors, start)?;
            let pair = candidate
                .as_ref()
                .and_then(|candidate| self.pair(location, candidate));
            slots.push(Slot {
                location,
                candidate,
                pair,
            });
        }
        let heads = self.transitions(&mut slots, kind, map, window.start, transitions)?;
        for slot in slots {
            if let Some(candidate) = slot.candidate {
                self.store(&mut block, kind, 0, slot.location, slot.pair, candidate)?;
            }
        }
        for (location, pair, head) in heads {
            self.store(&mut block, kind, 1, location, pair, head)?;
        }
        for (index, extra) in block.lanes(kind).iter_mut().enumerate().skip(1) {
            extra.name = lane_name(name, index);
        }
        Ok(block)
    }

    fn limit(
        &self,
        location: Location<'_>,
        next: Option<&Location<'_>>,
        window: &Range<i64>,
    ) -> Result<i64, Refusal> {
        let Some(next) = next else {
            return Ok(window.end);
        };
        let limit = if next.start > location.end {
            next.start.frame(self.project)?.0
        } else {
            next.end.floor_frame(self.project)?.0
        };
        Ok(limit.min(window.end))
    }

    fn item(
        &mut self,
        location: Location<'_>,
        kind: TrackKind,
        map: ContainerTime,
        start: i64,
        limit: i64,
        notes: &mut Vec<Loss>,
    ) -> Result<Option<Candidate>, Refusal> {
        if location.start < Time::ZERO {
            let loss = if location.end <= Time::ZERO {
                Loss::dropped(LossCode::BeforeStartTrimmed)
            } else {
                Loss::approximation(LossCode::BeforeStartTrimmed)
            };
            notes.push(loss);
            if location.end <= Time::ZERO {
                return Ok(None);
            }
        }
        match &location.placed.item {
            Item::Clip(clip) => {
                let Some(asset) = self.assets.info.get(clip.media.0).copied().flatten() else {
                    return Err(Refusal::new(
                        RefusalCode::Malformed,
                        "A clip references an unavailable IR media entry.",
                    ));
                };
                if asset.ranges_merged && location.placed.counts_item {
                    notes.push(Loss::approximation(LossCode::MediaRangesMerged));
                }
                let id = self.id();
                clips::decide(
                    &ClipInput {
                        raw: clip,
                        asset,
                        kind,
                        project: self.project,
                        transform_speed: map.speed,
                        source_from: location.start,
                        wanted_end: location.end,
                        start,
                        limit,
                    },
                    id,
                    &mut self.report,
                    notes,
                )
            }
            Item::Title { text, enabled, .. } if kind == TrackKind::Video => {
                let end = location.end.frame(self.project)?.0.min(limit);
                if end <= start {
                    notes.push(Loss::dropped(LossCode::ClipTooShort));
                    return Ok(None);
                }
                let title: String = text.chars().take(512).collect();
                if title.len() < text.len() {
                    notes.push(Loss::approximation(LossCode::TitleTextTruncated));
                }
                notes.push(Loss::approximation(LossCode::TitleStyleDefault));
                let mut clip = clips::blank(
                    self.id(),
                    ClipContent::Title(Title {
                        text: title,
                        ..Title::default()
                    }),
                    start,
                    TimeCode::ZERO..TimeCode(end - start),
                );
                clip.enabled = *enabled;
                Ok(Some(Candidate {
                    clip,
                    end,
                    asset: None,
                    head: false,
                }))
            }
            Item::Unsupported { what, .. } => {
                notes.push(Loss::dropped(LossCode::UnknownItem).named(what));
                Ok(None)
            }
            Item::Title { .. } => {
                notes.push(Loss::dropped(LossCode::UnknownItem).named("audio track title"));
                Ok(None)
            }
            _ => Ok(None),
        }
    }

    fn nested(
        &mut self,
        location: Location<'_>,
        stack: &Stack,
        outer: ContainerTime,
        window: &Range<i64>,
        depth: usize,
        losses: &mut Vec<Loss>,
    ) -> Result<Option<Block>, Refusal> {
        let site = &location.placed.site;
        if stack.time.speed <= Time::ZERO {
            let count =
                location.placed.item.population() + location.placed.decorations.population();
            self.report.item(site, count);
            self.report.loss_many(
                &Loss::dropped(LossCode::NestRetimeUnsupported),
                site,
                window.start,
                None,
                count,
            );
            return Ok(None);
        }
        self.report.item(site, 1);
        losses.push(Loss::approximation(LossCode::NestFlattened));
        let offset = location
            .start
            .checked_add(stack.time.offset.checked_div(outer.speed)?)?;
        let map = ContainerTime {
            offset,
            in_point: stack.time.in_point,
            speed: outer.speed.checked_mul(stack.time.speed)?,
        };
        Ok(Some(self.stack(stack, map, window, depth + 1)?))
    }

    fn anchor_blocks(
        &mut self,
        placed: &Placed,
        map: ContainerTime,
        window: &Range<i64>,
        depth: usize,
        anchors: &mut BTreeMap<i64, Block>,
        at: i64,
    ) -> Result<(), Refusal> {
        for anchor in &placed.anchors {
            let block = self.track(
                &anchor.track,
                map.compose(anchor.time)?,
                window,
                depth + 1,
                &mut StackTransitions::default(),
            )?;
            if anchors.entry(anchor.lane).or_default().overlay(block)? {
                self.report.loss(
                    &Loss::approximation(LossCode::LaneAdded),
                    &placed.site,
                    at,
                    None,
                );
            }
        }
        Ok(())
    }

    fn transitions<'a>(
        &mut self,
        slots: &mut [Slot<'a>],
        kind: TrackKind,
        map: ContainerTime,
        horizon: i64,
        transitions: &mut StackTransitions<'a>,
    ) -> Result<Vec<(Location<'a>, Option<Pair>, Candidate)>, Refusal> {
        let mut heads = Vec::new();
        let neighbors: Vec<_> = slots
            .iter()
            .enumerate()
            .filter(|(_, slot)| slot.location.end > slot.location.start)
            .map(|(index, _)| index)
            .collect();
        for index in 0..slots.len() {
            let placed = slots[index].location.placed;
            let Item::Transition {
                kind: name,
                before,
                after,
            } = &placed.item
            else {
                continue;
            };
            if self.format == InterchangeFormat::Fcpxml
                && !placed.counts_item
                && !transitions.applied_video.contains(placed.site.as_str())
            {
                continue;
            }
            let mut losses = Vec::new();
            let (before, after) = (
                before.checked_div(map.speed)?.frame(self.project)?.0.max(0),
                after.checked_div(map.speed)?.frame(self.project)?.0.max(0),
            );
            let right = neighbors.partition_point(|neighbor| *neighbor < index);
            let (previous, remaining) = slots.split_at_mut(index);
            let (slot, following) = remaining
                .split_first_mut()
                .ok_or_else(Refusal::arithmetic)?;
            let cut = slot.location.start;
            let a = right
                .checked_sub(1)
                .and_then(|left| previous.get_mut(neighbors[left]))
                .filter(|slot| slot.location.end == cut);
            let b = neighbors
                .get(right)
                .and_then(|&right| following.get_mut(right - index - 1))
                .filter(|slot| slot.location.start == cut);
            let nesting = a
                .as_ref()
                .is_some_and(|a| matches!(a.location.placed.item, Item::Nested { .. }))
                || b.as_ref()
                    .is_some_and(|b| matches!(b.location.placed.item, Item::Nested { .. }));
            let frames = before.saturating_add(after).unsigned_abs();
            if transitions.otio.record(placed, frames) {
                self.report.mark_item(&placed.site, Fidelity::Approximated);
            } else if nesting {
                losses.push(
                    Loss::approximation(LossCode::TransitionAtNestCut)
                        .bounded(LossBound::Frames(frames)),
                );
            } else {
                let input = TransitionInput {
                    kind,
                    name,
                    project: self.project,
                    before,
                    after,
                    horizon,
                    head_id: self.id(),
                };
                let pair = b.as_ref().and_then(|slot| slot.pair);
                let location = b.as_ref().map(|slot| slot.location);
                let mut left_none = None;
                let mut right_none = None;
                let left = a.map_or(&mut left_none, |slot| &mut slot.candidate);
                let right = b.map_or(&mut right_none, |slot| &mut slot.candidate);
                if let Some(applied) =
                    lower_same_track_transition(&input, left, right, &mut losses)?
                {
                    if self.format == InterchangeFormat::Fcpxml && kind == TrackKind::Video {
                        transitions.applied_video.insert(placed.site.as_str());
                    }
                    if let Some(head) = applied.head
                        && let Some(location) = location
                    {
                        heads.push((location, pair, head));
                    }
                }
            }
            self.losses(
                placed.notes.iter().chain(&losses),
                &placed.site,
                cut.frame(self.project)?.0,
                None,
            );
        }
        Ok(heads)
    }

    fn store(
        &mut self,
        block: &mut Block,
        kind: TrackKind,
        preferred: usize,
        location: Location<'_>,
        pair: Option<Pair>,
        candidate: Candidate,
    ) -> Result<(), Refusal> {
        let lanes = block.lanes(kind);
        let start = candidate.clip.timeline_start.0;
        let end = candidate.end;
        let index = (preferred..lanes.len())
            .find(|&index| lanes[index].fits(start, end))
            .unwrap_or(lanes.len().max(preferred));
        if index >= MAX_TRACKS_PER_KIND {
            return Err(too_many_tracks());
        }
        if index != preferred {
            self.report.loss(
                &Loss::approximation(LossCode::LaneAdded),
                &location.placed.site,
                start,
                Some(candidate.clip.id),
            );
        }
        if index >= lanes.len() {
            lanes.resize_with(index + 1, Lane::default);
        }
        lanes[index].clips.insert(
            start,
            Stored {
                candidate,
                pair,
                source: location.placed.site.clone(),
                raw_end: location.end,
            },
        );
        Ok(())
    }

    fn losses<'a>(
        &mut self,
        losses: impl IntoIterator<Item = &'a Loss>,
        site: &str,
        at: i64,
        clip: Option<ClipId>,
    ) {
        for loss in losses {
            self.report.loss(loss, site, at, clip);
        }
    }
}

fn distribute(locations: Vec<Location<'_>>) -> Result<Vec<Vec<Location<'_>>>, Refusal> {
    let mut lanes = vec![(None, Vec::new())];
    for location in locations {
        let index = if location.end == location.start {
            lanes
                .iter()
                .position(|(end, _)| *end == Some(location.start))
                .unwrap_or_default()
        } else {
            lanes
                .iter()
                .position(|(end, _)| end.is_none_or(|end| end <= location.start))
                .unwrap_or(lanes.len())
        };
        if index >= MAX_TRACKS_PER_KIND {
            return Err(too_many_tracks());
        }
        if index == lanes.len() {
            lanes.push((None, Vec::new()));
        }
        let (end, entries) = &mut lanes[index];
        if location.end > location.start {
            *end = Some(location.end);
        }
        entries.push(location);
    }
    Ok(lanes.into_iter().map(|(_, locations)| locations).collect())
}

fn lane_name(name: Option<&str>, lane: usize) -> Option<String> {
    name.and_then(|name| {
        let name = if lane == 0 {
            name.to_owned()
        } else {
            format!("{name} +{lane}")
        };
        (name.len() <= TRACK_NAME_MAX_BYTES).then_some(name)
    })
}
