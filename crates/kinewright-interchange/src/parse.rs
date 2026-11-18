use std::ops::Range;

use kinewright_core::LossCode;

use crate::{
    MAX_INPUT_BYTES, MAX_ITEMS, MAX_TEXT_BYTES, Refusal, RefusalCode, Time,
    ir::{Decorations, Discarded, Item, Loss, Marker, Placed, Stack, Track},
    shorten,
};

pub(crate) fn check_bytes(bytes: &[u8]) -> Result<(), Refusal> {
    if bytes.len() > MAX_INPUT_BYTES {
        return Err(Refusal::new(
            RefusalCode::TooLarge,
            "The timeline exceeds 128 MiB.",
        ));
    }
    Ok(())
}

pub(crate) fn too_many_items() -> Refusal {
    Refusal::new(
        RefusalCode::TooManyItems,
        "The import exceeds an item or media limit.",
    )
}

pub(crate) fn too_deep() -> Refusal {
    Refusal::new(
        RefusalCode::TooDeep,
        "The import exceeds its nesting limit.",
    )
}

pub(crate) fn malformed(at: impl Into<String>, message: &'static str) -> Refusal {
    Refusal::new(RefusalCode::Malformed, message).at(at)
}

pub(crate) fn select_timeline<T>(
    timelines: &[T],
    requested: Option<&str>,
    name: impl Fn(&T) -> &str,
) -> Result<usize, Refusal> {
    match (timelines, requested) {
        ([], _) => Err(Refusal::new(
            RefusalCode::NothingToImport,
            "The file contains no timelines.",
        )),
        ([_], None) => Ok(0),
        (_, requested) => {
            if let Some(requested) = requested {
                let mut matches = timelines
                    .iter()
                    .enumerate()
                    .filter(|(_, timeline)| name(timeline) == requested);
                match (matches.next(), matches.next()) {
                    (Some((index, _)), None) => return Ok(index),
                    (None, _) => {
                        return Err(Refusal::new(
                            RefusalCode::NotFound,
                            "The selected timeline was not found.",
                        ));
                    }
                    _ => {}
                }
            }
            let mut refusal = Refusal::new(
                RefusalCode::AmbiguousTimeline,
                "Select one of the timelines in this file.",
            );
            refusal.timelines = timelines
                .iter()
                .take(16)
                .map(|timeline| shorten(name(timeline), 128).into())
                .collect();
            Err(refusal)
        }
    }
}

pub(crate) fn text(value: &str) -> String {
    shorten(value, MAX_TEXT_BYTES).into()
}

pub(crate) fn apply_window(stack: &mut Stack, window: Range<Time>) -> Result<(), Refusal> {
    for track in &mut stack.tracks {
        window_track(track, &window)?;
    }
    window_decorations(&mut stack.decorations, &window)?;
    stack.window = Some(window);
    Ok(())
}

pub(crate) fn window_track(track: &mut Track, window: &Range<Time>) -> Result<(), Refusal> {
    for placed in &mut track.items {
        window_item(placed, window)?;
    }
    window_decorations(&mut track.decorations, window)
}

fn window_item(placed: &mut Placed, window: &Range<Time>) -> Result<(), Refusal> {
    for anchor in &mut placed.anchors {
        window_track(&mut anchor.track, window)?;
    }
    if matches!(placed.item, Item::Discarded { .. }) {
        return Ok(());
    }
    if let Item::Transition {
        kind,
        before,
        after,
    } = &placed.item
    {
        let begin = placed.at.checked_sub(*before)?;
        let end = placed.at.checked_add(*after)?;
        if end <= window.start || begin >= window.end {
            discard(placed);
        } else if begin < window.start || end > window.end || !window.contains(&placed.at) {
            placed
                .notes
                .push(Loss::approximation(LossCode::OutsideWindow));
            placed
                .notes
                .push(Loss::approximation(LossCode::TransitionAtNestCut).named(kind));
            placed.item = Item::Gap {
                duration: Time::ZERO,
            };
        }
        placed.at = placed.at.max(window.start).checked_sub(window.start)?;
        return Ok(());
    }

    let end = placed.at.checked_add(placed.item.span())?;
    let begin = placed.at.max(window.start);
    let finish = end.min(window.end);
    if finish <= begin {
        discard(placed);
        placed.at = Time::ZERO;
        return Ok(());
    }
    if begin != placed.at || finish != end {
        let in_point = begin.checked_sub(placed.at)?;
        let out_point = finish.checked_sub(placed.at)?;
        let duration = finish.checked_sub(begin)?;
        window_decorations(&mut placed.decorations, &(in_point..out_point))?;
        match &mut placed.item {
            Item::Clip(clip) => {
                clip.source_start = clip
                    .source_start
                    .checked_add(in_point.checked_mul(clip.time.average()?)?)?;
                clip.span = duration;
                window_decorations(&mut clip.decorations, &(in_point..out_point))?;
                for key in &mut clip.gain_keys {
                    key.at = key.at.checked_sub(in_point)?;
                }
            }
            Item::Nested {
                stack,
                duration: span,
            } => {
                if stack.time.speed > Time::ZERO {
                    let local = stack.time.in_local(in_point)?..stack.time.in_local(out_point)?;
                    apply_window(stack, local)?;
                    stack.time.offset = Time::ZERO;
                    stack.time.in_point = Time::ZERO;
                }
                *span = duration;
            }
            Item::Title {
                duration: span,
                decorations,
                ..
            } => {
                *span = duration;
                window_decorations(decorations, &(in_point..out_point))?;
            }
            Item::Gap { duration: span } | Item::Unsupported { duration: span, .. } => {
                *span = duration;
            }
            Item::Transition { .. } | Item::Discarded { .. } => {}
        }
        placed
            .notes
            .push(Loss::approximation(LossCode::OutsideWindow));
    }
    placed.at = begin.checked_sub(window.start)?;
    Ok(())
}

fn discard(placed: &mut Placed) {
    placed.item = Item::Discarded {
        count: if placed.counts_item {
            placed.item.population() + placed.decorations.population()
        } else {
            0
        },
        loss: Loss::dropped(LossCode::OutsideWindow),
    };
    placed.notes.clear();
    placed.decorations = Decorations::empty();
}

fn window_decorations(decorations: &mut Decorations, window: &Range<Time>) -> Result<(), Refusal> {
    let mut kept = Vec::with_capacity(decorations.markers.len());
    for marker in std::mem::take(&mut decorations.markers) {
        if let Some(marker) = trim_marker(marker, window, &mut decorations.removed)? {
            kept.push(marker);
        }
    }
    decorations.markers = kept;
    Ok(())
}

fn trim_marker(
    mut marker: Marker,
    window: &Range<Time>,
    removed: &mut Vec<Discarded>,
) -> Result<Option<Marker>, Refusal> {
    let end = marker.at.checked_add(marker.duration)?;
    let begin = marker.at.max(window.start);
    let finish = end.min(window.end);
    if finish < begin || (finish == begin && !window.contains(&marker.at)) {
        removed.push(Discarded {
            count: 1,
            loss: Loss::dropped(LossCode::OutsideWindow),
            site: marker.site,
        });
        return Ok(None);
    }
    marker.at = begin.checked_sub(window.start)?;
    marker.duration = finish.checked_sub(begin)?;
    Ok(Some(marker))
}

#[derive(Default)]
pub(crate) struct Budget {
    count: usize,
}

impl Budget {
    pub fn item(&mut self) -> Result<(), Refusal> {
        self.count += 1;
        if self.count > MAX_ITEMS {
            return Err(too_many_items());
        }
        Ok(())
    }
}
