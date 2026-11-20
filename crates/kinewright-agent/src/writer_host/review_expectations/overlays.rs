use super::{
    ReviewExpectationError, ReviewExpectationInput, add_frame_work, bound, clip_end, inventory,
    output_frames, parse_digest, pin_for, times,
};
use kinewright_core::output_review::{
    AbsenceProof, MAX_REVIEW_BOUNDS_RUNS, OverlayBoundsRun, OverlayExpectations, OverlayKind,
    OverlayPixels, OverlayText, OverlayUnknown, ReviewAnnotations, ReviewOverlay,
};
use kinewright_core::{
    Clip, ClipContent, Document, FrameRounding, MediaKind, TimeCode, TitlePixelBounds, TrackKind,
    TransitionShading, map_frames_with_rounding, title_layout, transition_descriptor,
};
use kinewright_media::{OverlayGeometry, review_overlay_geometry};
use std::{collections::BTreeSet, ops::Range};

pub(super) fn element_kind(document: &Document, clip: &Clip) -> Option<OverlayKind> {
    match &clip.content {
        ClipContent::Title(title) if title.caption_preset.is_some() => Some(OverlayKind::Caption),
        ClipContent::Title(_) => Some(OverlayKind::Title),
        ClipContent::Media | ClipContent::Freeze(_)
            if document
                .asset(clip.asset)
                .is_some_and(|asset| asset.kind == MediaKind::Image) =>
        {
            Some(OverlayKind::Card)
        }
        _ => None,
    }
}

pub(super) fn derive(
    input: &ReviewExpectationInput<'_>,
    annotations: &ReviewAnnotations,
) -> Result<OverlayExpectations, ReviewExpectationError> {
    let document = input.document;
    let mut unmatched: BTreeSet<_> = annotations.sourced_elements.iter().copied().collect();
    let mut text_cards: BTreeSet<_> = annotations.text_cards.iter().copied().collect();
    let mut captions = Vec::new();
    let mut titles = Vec::new();
    let mut cards = Vec::new();
    let mut frame_work = 0;
    let mut bounds_runs = 0;
    let pushes = push_intervals(document)?;
    for (track_index, track) in document.tracks.iter().enumerate() {
        if track.kind != TrackKind::Video {
            continue;
        }
        for clip in &track.clips {
            let Some(kind) = element_kind(document, clip) else {
                continue;
            };
            let sourced = unmatched.remove(&clip.id);
            let kind = if text_cards.remove(&clip.id) {
                if kind != OverlayKind::Title {
                    return Err(ReviewExpectationError::AnnotationTextCard(clip.id));
                }
                OverlayKind::Card
            } else {
                kind
            };
            let project = clip.timeline_start..clip_end(document, clip)?;
            let frames = output_frames(project.clone(), document.fps, input.settings.fps)?;
            if frames.is_empty() || !clip.enabled && clip.enabled_curve.is_none() {
                continue;
            }
            let backdrop: Vec<_> = pushes
                .iter()
                .filter_map(|(upper_index, range)| {
                    (*upper_index > track_index
                        && range.start < project.end
                        && project.start < range.end)
                        .then_some(range)
                })
                .collect();
            let expectation = overlay(
                input,
                clip,
                kind,
                sourced,
                frames,
                &backdrop,
                &mut frame_work,
                &mut bounds_runs,
            )?;
            if let Some(overlay) = expectation {
                match kind {
                    OverlayKind::Caption => captions.push(overlay),
                    OverlayKind::Title => titles.push(overlay),
                    OverlayKind::Card => cards.push(overlay),
                }
            }
        }
    }
    if let Some(&id) = unmatched.first() {
        return Err(ReviewExpectationError::AnnotationElement(id));
    }
    if let Some(&id) = text_cards.first() {
        return Err(ReviewExpectationError::AnnotationElement(id));
    }
    let ordered = |mut elements: Vec<ReviewOverlay>| {
        elements.sort_unstable_by_key(|item| (item.cue.start, item.element));
        inventory(elements, true, AbsenceProof::NoDocumentElementsOnOutput)
    };
    Ok(OverlayExpectations {
        captions: ordered(captions),
        titles: ordered(titles),
        cards: ordered(cards),
    })
}

fn push_intervals(
    document: &Document,
) -> Result<Vec<(usize, Range<TimeCode>)>, ReviewExpectationError> {
    let mut intervals = Vec::new();
    for (index, track) in document
        .tracks
        .iter()
        .enumerate()
        .filter(|(_, track)| track.kind == TrackKind::Video)
    {
        for clip in &track.clips {
            if let Some(transition) = &clip.transition_in
                && transition.duration.0 > 1
                && (clip.enabled || clip.enabled_curve.is_some())
                && transition_descriptor(&transition.name)
                    .is_some_and(|desc| matches!(desc.shading, TransitionShading::Push { .. }))
            {
                let end = clip
                    .timeline_start
                    .checked_add(TimeCode(transition.duration.0 - 1))
                    .ok_or(kinewright_core::OpError::TimeOverflow)?;
                intervals.push((
                    index,
                    clip.timeline_start..end.min(clip_end(document, clip)?),
                ));
            }
        }
    }
    Ok(intervals)
}

fn base(
    input: &ReviewExpectationInput<'_>,
    clip: &Clip,
) -> (
    OverlayText,
    OverlayPixels,
    Result<TitlePixelBounds, OverlayUnknown>,
) {
    let raster = input.settings.resolution;
    if let ClipContent::Title(title) = &clip.content {
        return match title_layout(title, raster) {
            Some(layout) => (
                OverlayText::Expected(title.text.clone()),
                OverlayPixels::RenderedText,
                Ok(layout.visual_bounds),
            ),
            None => (
                OverlayText::Unknown(OverlayUnknown::TextLayoutUnavailable),
                OverlayPixels::Unknown(OverlayUnknown::TextLayoutUnavailable),
                Err(OverlayUnknown::TextLayoutUnavailable),
            ),
        };
    }
    // The compositor maps the decoded still onto a full-raster quad before
    // effects. Keep its pinned input for a pixel check even with unknown text.
    let pixels = input
        .document
        .asset(clip.asset)
        .and_then(|asset| pin_for(input.input_manifest, asset))
        .and_then(|entry| {
            Some(OverlayPixels::PinnedImage {
                sha256: parse_digest(&entry.object.sha256)?,
                byte_length: entry.object.byte_length,
            })
        })
        .unwrap_or(OverlayPixels::Unknown(OverlayUnknown::ImageNotPinned));
    let bounds = TitlePixelBounds {
        left: 0,
        top: 0,
        right: i32::try_from(raster.0).expect("review raster is bounded"),
        bottom: i32::try_from(raster.1).expect("review raster is bounded"),
    };
    (
        OverlayText::Unknown(OverlayUnknown::OpaqueCard),
        pixels,
        Ok(bounds),
    )
}

#[allow(
    clippy::too_many_arguments,
    reason = "one overlay and its aggregate work budgets"
)]
fn overlay(
    input: &ReviewExpectationInput<'_>,
    clip: &Clip,
    kind: OverlayKind,
    sourced: bool,
    frames: Range<TimeCode>,
    backdrop: &[&Range<TimeCode>],
    frame_work: &mut usize,
    bounds_runs: &mut usize,
) -> Result<Option<ReviewOverlay>, ReviewExpectationError> {
    let (text, pixels, base) = base(input, clip);
    let dynamic = !backdrop.is_empty()
        || clip.enabled_curve.is_some()
        || clip.transition_in.is_some()
        || clip
            .effects
            .iter()
            .any(|fx| fx.enabled_curve.is_some() || !fx.keyframes.is_empty())
        || matches!(&clip.content, ClipContent::Title(title) if title.fade_in_frames.0 > 1 || title.fade_out_frames.0 > 1);
    let document = input.document;
    let rate = input.settings.fps;
    let cue = times(clip.timeline_start..clip_end(document, clip)?, document.fps)?;
    let mut visible: Vec<OverlayBoundsRun> = Vec::new();
    let mut enabled_on_output = false;
    let end = if dynamic {
        frames.end.0
    } else {
        frames.start.0 + 1
    };
    *frame_work = add_frame_work(*frame_work, end - frames.start.0)?;
    for frame in frames.start.0..end {
        let at =
            map_frames_with_rounding(TimeCode(frame), rate, document.fps, FrameRounding::Floor)?;
        let local = TimeCode(at.0 - clip.timeline_start.0);
        if !clip.is_enabled_at(local) {
            continue;
        }
        enabled_on_output = true;
        let bounds = match base {
            Ok(bounds) => match review_overlay_geometry(
                document,
                clip,
                at,
                bounds,
                input.settings.resolution,
            )? {
                OverlayGeometry::Hidden => continue,
                OverlayGeometry::Visible(bounds)
                    if !backdrop.iter().any(|range| range.contains(&at)) =>
                {
                    Ok(bounds)
                }
                OverlayGeometry::Visible(_) | OverlayGeometry::Unresolved => {
                    Err(OverlayUnknown::GeometryUnavailable)
                }
            },
            Err(reason) => Err(reason),
        };
        let next = if dynamic {
            TimeCode(frame + 1)
        } else {
            frames.end
        };
        let output = times(TimeCode(frame)..next, rate)?;
        if let Some(last) = visible
            .last_mut()
            .filter(|last| last.bounds == bounds && last.output.end == output.start)
        {
            last.output.end = output.end;
        } else {
            *bounds_runs += 1;
            bound("overlay_bounds_runs", *bounds_runs, MAX_REVIEW_BOUNDS_RUNS)?;
            visible.push(OverlayBoundsRun { output, bounds });
        }
    }
    if visible.is_empty() {
        if !enabled_on_output {
            return Ok(None);
        }
        *bounds_runs += 1;
        bound("overlay_bounds_runs", *bounds_runs, MAX_REVIEW_BOUNDS_RUNS)?;
        visible.push(OverlayBoundsRun {
            output: times(frames, rate)?,
            bounds: Err(OverlayUnknown::NotVisible),
        });
    }
    Ok(Some(ReviewOverlay {
        element: clip.id,
        kind,
        cue,
        visible,
        text,
        pixels,
        sourced,
    }))
}
