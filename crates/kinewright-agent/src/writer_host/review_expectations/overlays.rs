use super::{
    DerivationWork, ReviewExpectationError, ReviewExpectationInput, bound, clip_end, inventory,
    output_frames, parse_digest, pin_for, times,
};
use kinewright_core::output_review::{
    AbsenceProof, MAX_REVIEW_BOUNDS_RUNS, OffOutputAnnotation, OverlayBoundsRun,
    OverlayExpectations, OverlayKind, OverlayPixels, OverlayText, OverlayUnknown,
    ReviewAnnotations, ReviewOverlay,
};
use kinewright_core::{
    Clip, ClipContent, Document, FrameRounding, MediaKind, TimeCode, TitlePixelBounds, TrackKind,
    map_frames_with_rounding, title_layout,
};
use kinewright_media::{
    OverlayGeometry, review_layer_can_reshape_below, review_layer_reshapes_below_at,
    review_overlay_geometry,
};
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
    work: &mut DerivationWork,
) -> Result<OverlayExpectations, ReviewExpectationError> {
    let document = input.document;
    let mut unmatched: BTreeSet<_> = annotations.sourced_elements.iter().copied().collect();
    let mut text_cards: BTreeSet<_> = annotations.text_cards.iter().copied().collect();
    let mut annotated_off_output = Vec::new();
    let upper = upper_layers(document)?;
    let mut plans = Vec::new();
    for (track_index, track) in document.tracks.iter().enumerate() {
        if track.kind != TrackKind::Video {
            continue;
        }
        for clip in &track.clips {
            let Some(kind) = element_kind(document, clip) else {
                continue;
            };
            let annotation = OffOutputAnnotation {
                element: clip.id,
                sourced: unmatched.remove(&clip.id),
                text_card: text_cards.remove(&clip.id),
            };
            let kind = if annotation.text_card {
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
                retain_off_output(annotation, &mut annotated_off_output);
                continue;
            }
            work.charge(upper.len())?;
            let backdrop: Vec<_> = upper
                .iter()
                .filter(|layer| {
                    layer.track > track_index
                        && layer.project.start < project.end
                        && project.start < layer.project.end
                })
                .collect();
            let dynamic = dynamic_overlay(clip) || !backdrop.is_empty();
            let evaluations = if dynamic {
                usize::try_from(frames.end.0 - frames.start.0).unwrap_or(usize::MAX)
            } else {
                1
            };
            let cost = backdrop.iter().fold(work.frame_cost(clip), |cost, layer| {
                cost.saturating_add(work.frame_cost(layer.clip))
            });
            work.charge(evaluations.saturating_mul(cost))?;
            plans.push(OverlayPlan {
                clip,
                kind,
                annotation,
                frames,
                dynamic,
                backdrop,
            });
        }
    }
    if let Some(&id) = unmatched.first().or_else(|| text_cards.first()) {
        return Err(ReviewExpectationError::AnnotationElement(id));
    }

    let mut captions = Vec::new();
    let mut titles = Vec::new();
    let mut cards = Vec::new();
    let mut bounds_runs = 0;
    for plan in plans {
        let expectation = overlay(input, &plan, &mut bounds_runs)?;
        if expectation.as_ref().is_none_or(|overlay| {
            overlay
                .visible
                .iter()
                .all(|run| run.bounds == Err(OverlayUnknown::NotVisible))
        }) {
            retain_off_output(plan.annotation, &mut annotated_off_output);
        }
        if let Some(overlay) = expectation {
            match plan.kind {
                OverlayKind::Caption => captions.push(overlay),
                OverlayKind::Title => titles.push(overlay),
                OverlayKind::Card => cards.push(overlay),
            }
        }
    }
    let ordered = |mut elements: Vec<ReviewOverlay>| {
        elements.sort_unstable_by_key(|item| (item.cue.start, item.element));
        inventory(elements, true, AbsenceProof::NoDocumentElementsOnOutput)
    };
    annotated_off_output.sort_unstable_by_key(|item| item.element);
    Ok(OverlayExpectations {
        annotated_off_output,
        captions: ordered(captions),
        titles: ordered(titles),
        cards: ordered(cards),
    })
}

fn retain_off_output(annotation: OffOutputAnnotation, output: &mut Vec<OffOutputAnnotation>) {
    if annotation.sourced || annotation.text_card {
        output.push(annotation);
    }
}

struct UpperLayer<'a> {
    track: usize,
    clip: &'a Clip,
    project: Range<TimeCode>,
}

fn upper_layers(document: &Document) -> Result<Vec<UpperLayer<'_>>, ReviewExpectationError> {
    let mut layers = Vec::new();
    for (track, entry) in document.tracks.iter().enumerate() {
        if entry.kind != TrackKind::Video {
            continue;
        }
        for clip in &entry.clips {
            if (clip.enabled || clip.enabled_curve.is_some())
                && review_layer_can_reshape_below(clip)
            {
                layers.push(UpperLayer {
                    track,
                    clip,
                    project: clip.timeline_start..clip_end(document, clip)?,
                });
            }
        }
    }
    Ok(layers)
}

struct OverlayPlan<'a> {
    clip: &'a Clip,
    kind: OverlayKind,
    annotation: OffOutputAnnotation,
    frames: Range<TimeCode>,
    dynamic: bool,
    backdrop: Vec<&'a UpperLayer<'a>>,
}

fn dynamic_overlay(clip: &Clip) -> bool {
    clip.enabled_curve.is_some()
        || clip.transition_in.is_some()
        || clip
            .effects
            .iter()
            .any(|fx| fx.enabled_curve.is_some() || !fx.keyframes.is_empty())
        || matches!(&clip.content, ClipContent::Title(title) if title.fade_in_frames.0 > 1 || title.fade_out_frames.0 > 1)
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

fn overlay(
    input: &ReviewExpectationInput<'_>,
    plan: &OverlayPlan<'_>,
    bounds_runs: &mut usize,
) -> Result<Option<ReviewOverlay>, ReviewExpectationError> {
    let clip = plan.clip;
    let frames = &plan.frames;
    let (text, pixels, base) = base(input, clip);
    let document = input.document;
    let rate = input.settings.fps;
    let cue = times(clip.timeline_start..clip_end(document, clip)?, document.fps)?;
    let mut visible: Vec<OverlayBoundsRun> = Vec::new();
    let mut enabled_on_output = false;
    let end = if plan.dynamic {
        frames.end.0
    } else {
        frames.start.0 + 1
    };
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
                OverlayGeometry::Visible(bounds) => {
                    if plan.backdrop.iter().any(|layer| {
                        layer.project.contains(&at)
                            && review_layer_reshapes_below_at(layer.clip, at)
                    }) {
                        Err(OverlayUnknown::UpperLayerGeometry)
                    } else {
                        Ok(bounds)
                    }
                }
                OverlayGeometry::Unresolved => Err(OverlayUnknown::GeometryUnavailable),
            },
            Err(reason) => Err(reason),
        };
        let next = if plan.dynamic {
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
            output: times(frames.clone(), rate)?,
            bounds: Err(OverlayUnknown::NotVisible),
        });
    }
    Ok(Some(ReviewOverlay {
        element: clip.id,
        kind: plan.kind,
        cue,
        visible,
        text,
        pixels,
        sourced: plan.annotation.sourced,
    }))
}
