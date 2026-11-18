use std::collections::{BTreeMap, BTreeSet};

use kinewright_core::TrackKind;

use crate::{
    MAX_NEST_DEPTH, Refusal, Time,
    ir::{ContainerTime, Item, MediaId, Placed, Stack, Track},
    parse,
};

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct Source {
    media: MediaId,
    start: Time,
    in_point: Time,
}

struct PictureCut<'a> {
    site: &'a str,
    outgoing: BTreeSet<Source>,
    incoming: BTreeSet<Source>,
}

#[derive(Default)]
pub(super) struct SharedCuts<'a> {
    audio_to_picture: BTreeMap<&'a str, &'a str>,
    pub frames: BTreeMap<Time, BTreeMap<&'a str, u64>>,
}

impl<'a> SharedCuts<'a> {
    pub fn for_stack(stack: &'a Stack) -> Result<Self, Refusal> {
        let mut shared = Self::default();
        let mut audio = stack
            .tracks
            .iter()
            .filter(|track| track.kind == TrackKind::Audio)
            .flat_map(track_cuts)
            .peekable();
        if audio.peek().is_none() {
            return Ok(shared);
        }
        let picture = picture_cuts(stack)?;
        for (transition, left, right) in audio {
            let (Some(left), Some(right)) =
                (clip_source(left, left.at), clip_source(right, right.at))
            else {
                continue;
            };
            if let Some(cut) = picture.get(&transition.at).and_then(|cuts| {
                cuts.iter()
                    .find(|cut| cut.outgoing.contains(&left) && cut.incoming.contains(&right))
            }) {
                shared.audio_to_picture.insert(&transition.site, cut.site);
                shared
                    .frames
                    .entry(transition.at)
                    .or_default()
                    .entry(cut.site)
                    .or_default();
            }
        }
        Ok(shared)
    }

    pub fn record(&mut self, transition: &Placed, frames: u64) -> bool {
        let site = transition.site.as_str();
        let picture = self.audio_to_picture.get(site).copied().unwrap_or(site);
        if let Some(max_frames) = self
            .frames
            .get_mut(&transition.at)
            .and_then(|cuts| cuts.get_mut(picture))
        {
            *max_frames = (*max_frames).max(frames);
            true
        } else {
            false
        }
    }
}

fn track_cuts(track: &Track) -> impl Iterator<Item = (&Placed, &Placed, &Placed)> {
    let neighbors = || {
        track
            .items
            .iter()
            .enumerate()
            .filter(|(_, item)| item.item.span() > Time::ZERO)
    };
    neighbors()
        .zip(neighbors().skip(1))
        .flat_map(|((left_index, left), (right_index, right))| {
            track.items[left_index + 1..right_index]
                .iter()
                .filter_map(move |transition| {
                    (matches!(transition.item, Item::Transition { .. })
                        && transition.at == right.at
                        && left.at.checked_add(left.item.span()).ok() == Some(transition.at))
                    .then_some((transition, left, right))
                })
        })
}

fn picture_cuts(stack: &Stack) -> Result<BTreeMap<Time, Vec<PictureCut<'_>>>, Refusal> {
    let mut cuts = BTreeMap::<_, Vec<_>>::new();
    for track in stack
        .tracks
        .iter()
        .filter(|track| track.kind == TrackKind::Video)
    {
        let mut previous_cut = None;
        for (transition, left, right) in track_cuts(track) {
            if !matches!(left.item, Item::Nested { .. })
                && !matches!(right.item, Item::Nested { .. })
                || previous_cut == Some(transition.at)
            {
                continue;
            }
            previous_cut = Some(transition.at);
            let mut cut = PictureCut {
                site: &transition.site,
                outgoing: BTreeSet::new(),
                incoming: BTreeSet::new(),
            };
            let map = ContainerTime::default();
            boundary_sources(left, map, transition.at, true, 0, &mut cut.outgoing)?;
            boundary_sources(right, map, transition.at, false, 0, &mut cut.incoming)?;
            if !cut.outgoing.is_empty() && !cut.incoming.is_empty() {
                cuts.entry(transition.at).or_default().push(cut);
            }
        }
    }
    Ok(cuts)
}

fn clip_source(placed: &Placed, start: Time) -> Option<Source> {
    let Item::Clip(clip) = &placed.item else {
        return None;
    };
    Some(Source {
        media: clip.media,
        start,
        in_point: clip.source_start,
    })
}

fn boundary_sources(
    placed: &Placed,
    map: ContainerTime,
    at: Time,
    outgoing: bool,
    depth: usize,
    sources: &mut BTreeSet<Source>,
) -> Result<(), Refusal> {
    let start = map.to_parent(placed.at)?;
    let end = map.to_parent(placed.at.checked_add(placed.item.span())?)?;
    if start == end || (if outgoing { end } else { start }) != at {
        return Ok(());
    }
    if let Some(source) = clip_source(placed, start) {
        sources.insert(source);
    } else if let Item::Nested { stack, .. } = &placed.item
        && stack.time.speed > Time::ZERO
    {
        if depth >= MAX_NEST_DEPTH {
            return Err(parse::too_deep());
        }
        let inner = map.compose(ContainerTime {
            offset: placed.at.checked_add(stack.time.offset)?,
            ..stack.time
        })?;
        for track in stack
            .tracks
            .iter()
            .filter(|track| track.kind == TrackKind::Video)
        {
            for item in &track.items {
                boundary_sources(item, inner, at, outgoing, depth + 1, sources)?;
            }
        }
    }
    Ok(())
}
