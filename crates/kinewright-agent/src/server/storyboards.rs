//! Timeline storyboards, cut neighbourhoods, and contact-sheet rendering.

use super::*;

impl KinewrightMcp {
    pub(super) fn timeline_storyboard(
        &self,
        args: StoryboardArgs,
    ) -> Result<CallToolResult, McpError> {
        let (revision, document) = self.snapshot()?;
        self.storyboard_for_document(revision, &document, args, "timeline storyboard", None)
    }

    /// Render exact frames on both sides of contiguous media cuts.
    ///
    /// Uniform storyboards are intentionally poor at finding one-frame flashes
    /// and near-match jump cuts. This inspector keeps the cut-local evidence
    /// compact and maps every cell back to its exact project frame.
    #[allow(clippy::too_many_lines)]
    pub(super) fn cut_neighborhoods(
        &self,
        args: &CutNeighborhoodsArgs,
    ) -> Result<CallToolResult, McpError> {
        let (revision, document) = self.snapshot()?;
        if let Some(error) = self.document_availability_error(&document, "cut proof") {
            return Ok(error);
        }
        let Some(track) = document
            .tracks
            .iter()
            .find(|track| track.id == args.track_id)
        else {
            return Ok(error_text(format!(
                "track {} does not exist",
                args.track_id
            )));
        };
        if track.kind != TrackKind::Video {
            return Ok(error_text(format!(
                "track {} is not a video track",
                args.track_id
            )));
        }

        let frames_before = args.frames_before.unwrap_or(1);
        let frames_after = args.frames_after.unwrap_or(3);
        if !(1..=6).contains(&frames_before) || !(1..=6).contains(&frames_after) {
            return Ok(error_text(
                "frames_before and frames_after must be in 1..=6",
            ));
        }
        let cut_count = args.cut_count.unwrap_or(12);
        if !(1..=12).contains(&cut_count) {
            return Ok(error_text("cut_count must be in 1..=12"));
        }
        let max_width = args.max_width.unwrap_or(160);
        if !(64..=THUMBNAIL_MAX_WIDTH).contains(&max_width) {
            return Ok(error_text(format!(
                "max_width must be in 64..={THUMBNAIL_MAX_WIDTH}"
            )));
        }
        let maximum_secondary_change_basis_points = args
            .maximum_secondary_change_basis_points
            .unwrap_or(DEFAULT_MAXIMUM_CUT_SECONDARY_CHANGE_BASIS_POINTS);
        if maximum_secondary_change_basis_points > 10_000 {
            return Ok(error_text(
                "maximum_secondary_change_basis_points must be in 0..=10000",
            ));
        }

        let mut clips = track
            .clips
            .iter()
            .filter(|clip| clip.content.is_media())
            .collect::<Vec<_>>();
        clips.sort_by_key(|clip| (clip.timeline_start, clip.id));
        let mut cuts = Vec::new();
        for pair in clips.windows(2) {
            let outgoing = pair[0];
            let incoming = pair[1];
            let Some(outgoing_end) = document
                .clip_duration(outgoing)
                .ok()
                .and_then(|duration| outgoing.timeline_start.checked_add(duration))
            else {
                return Ok(error_text(format!(
                    "could not map clip {} duration",
                    outgoing.id
                )));
            };
            if outgoing_end == incoming.timeline_start {
                cuts.push((incoming.timeline_start, outgoing.id, incoming.id));
            }
        }

        let cut_offset = args.cut_offset.unwrap_or_default();
        if cut_offset > cuts.len() {
            return Ok(error_text(format!(
                "cut_offset {cut_offset} exceeds {} contiguous media cuts",
                cuts.len()
            )));
        }
        let selected = cuts
            .iter()
            .enumerate()
            .skip(cut_offset)
            .take(usize::from(cut_count))
            .collect::<Vec<_>>();
        if selected.is_empty() {
            return Ok(success_structured(
                format!(
                    "track {} has no selected contiguous media cuts",
                    args.track_id
                ),
                serde_json::json!({
                    "timeline_revision": revision.0,
                    "track_id": args.track_id.0,
                    "total_cut_count": cuts.len(),
                    "cut_offset": cut_offset,
                    "returned_cut_count": 0,
                    "cuts": [],
                    "cells": [],
                }),
            ));
        }

        let returned_cut_count = selected.len();
        let mut images = Vec::with_capacity(
            selected.len() * (usize::from(frames_before) + usize::from(frames_after)),
        );
        let mut cells = Vec::with_capacity(images.capacity());
        let mut cut_manifest = Vec::with_capacity(selected.len());
        let mut issues = Vec::new();
        for &(cut_index, &(cut_frame, outgoing_clip, incoming_clip)) in &selected {
            let first_cell = cells.len() + 1;
            let first_image = images.len();
            let mut offsets =
                Vec::with_capacity(usize::from(frames_before) + usize::from(frames_after));
            for offset in -i64::from(frames_before)..i64::from(frames_after) {
                let project_frame = TimeCode(cut_frame.0.saturating_add(offset));
                if project_frame < TimeCode::ZERO || project_frame >= document.duration {
                    continue;
                }
                match self.analysis.thumbnail_for_document(
                    Arc::clone(&document),
                    project_frame,
                    max_width,
                ) {
                    Ok(image) => images.push(image),
                    Err(error) => return Ok(error_text(error.to_string())),
                }
                offsets.push(offset);
                cells.push(serde_json::json!({
                    "cell": cells.len() + 1,
                    "cut_index": cut_index,
                    "cut_frame": cut_frame.0,
                    "project_frame": project_frame.0,
                    "offset_from_cut": offset,
                    "side": if offset < 0 { "outgoing" } else { "incoming" },
                }));
            }
            let changes = images[first_image..]
                .windows(2)
                .zip(offsets.windows(2))
                .map(|(pair, offsets)| {
                    let change_basis_points =
                        rgba_mean_absolute_difference_basis_points(&pair[0], &pair[1])
                            .unwrap_or(10_000);
                    let secondary_change = offsets[0] >= 0
                        && change_basis_points > maximum_secondary_change_basis_points;
                    if secondary_change {
                        issues.push(serde_json::json!({
                            "cut_index": cut_index,
                            "cut_frame": cut_frame.0,
                            "kind": "suspected_internal_cut_after_in_point",
                            "from_offset": offsets[0],
                            "to_offset": offsets[1],
                            "change_basis_points": change_basis_points,
                            "maximum_basis_points": maximum_secondary_change_basis_points,
                        }));
                    }
                    serde_json::json!({
                        "from_offset": offsets[0],
                        "to_offset": offsets[1],
                        "change_basis_points": change_basis_points,
                        "secondary_change": secondary_change,
                    })
                })
                .collect::<Vec<_>>();
            cut_manifest.push(serde_json::json!({
                "cut_index": cut_index,
                "project_frame": cut_frame.0,
                    "outgoing_clip_id": outgoing_clip.0,
                    "incoming_clip_id": incoming_clip.0,
                "first_cell": first_cell,
                "last_cell": cells.len(),
                "adjacent_changes": changes,
            }));
        }

        let sheet = compose_contact_sheet(&images)?;
        let png = encode_png(&sheet)?;
        let next_cut_offset = (cut_offset + returned_cut_count < cuts.len())
            .then_some(cut_offset + returned_cut_count);
        let manifest = serde_json::json!({
            "timeline_revision": revision.0,
            "track_id": args.track_id.0,
            "total_cut_count": cuts.len(),
            "cut_offset": cut_offset,
            "returned_cut_count": returned_cut_count,
            "next_cut_offset": next_cut_offset,
            "frames_before": frames_before,
            "frames_after": frames_after,
            "maximum_secondary_change_basis_points": maximum_secondary_change_basis_points,
            "clean": issues.is_empty(),
            "issue_count": issues.len(),
            "issues": issues,
            "cuts": cut_manifest,
            "cells": cells,
            "sheet": {"width": sheet.width, "height": sheet.height},
        });
        let status = if manifest["clean"] == true {
            "CUT EDGE REVIEW PASSED"
        } else {
            "CUT EDGE REVIEW FAILED"
        };
        let mut result = CallToolResult::success(vec![
            ContentBlock::text(format!("{status}: cut neighborhoods {manifest}")),
            ContentBlock::image(BASE64.encode(png), "image/png"),
        ]);
        result.structured_content = Some(manifest);
        Ok(result)
    }

    pub(super) fn storyboard_for_document(
        &self,
        revision: TimelineRevision,
        document: &Arc<Document>,
        args: StoryboardArgs,
        label: &str,
        variant: Option<serde_json::Value>,
    ) -> Result<CallToolResult, McpError> {
        if let Some(error) = self.document_availability_error(document, label) {
            return Ok(error);
        }
        let range = validated_timeline_range(document, args.range, label)?;
        let frame_count = args.frame_count.unwrap_or(STORYBOARD_DEFAULT_FRAMES);
        if !(1..=STORYBOARD_MAX_FRAMES).contains(&frame_count) {
            return Ok(error_text(format!(
                "frame_count must be in 1..={STORYBOARD_MAX_FRAMES}"
            )));
        }
        let max_width = args.max_width.unwrap_or(STORYBOARD_DEFAULT_CELL_WIDTH);
        if !(64..=THUMBNAIL_MAX_WIDTH).contains(&max_width) {
            return Ok(error_text(format!(
                "max_width must be in 64..={THUMBNAIL_MAX_WIDTH}"
            )));
        }
        let frames = storyboard_sample_frames(&range, frame_count);
        let mut images = Vec::with_capacity(frames.len());
        for frame in &frames {
            match self
                .analysis
                .thumbnail_for_document(Arc::clone(document), *frame, max_width)
            {
                Ok(image) => images.push(image),
                Err(error) => return Ok(error_text(error.to_string())),
            }
        }
        let sheet = compose_contact_sheet(&images)?;
        let png = encode_png(&sheet)?;
        let cells = frames
            .iter()
            .enumerate()
            .map(|(index, frame)| {
                serde_json::json!({
                    "cell": index + 1,
                    "project_frame": frame.0,
                })
            })
            .collect::<Vec<_>>();
        let mut manifest = serde_json::json!({
            "timeline_revision": revision.0,
            "range": {"start": range.start.0, "end": range.end.0},
            "cells": cells,
            "sheet": {"width": sheet.width, "height": sheet.height},
        });
        if let Some(variant) = variant {
            manifest["delivery_variant"] = variant;
        }
        let mut result = CallToolResult::success(vec![
            ContentBlock::text(format!("{label} {manifest}")),
            ContentBlock::image(BASE64.encode(png), "image/png"),
        ]);
        result.structured_content = Some(manifest);
        Ok(result)
    }
}

pub(super) fn storyboard_sample_frames(
    range: &std::ops::Range<TimeCode>,
    frame_count: u8,
) -> Vec<TimeCode> {
    let count = usize::from(frame_count.max(1));
    let inclusive_span = range.end.0.saturating_sub(range.start.0).saturating_sub(1);
    if count == 1 {
        return vec![TimeCode(range.start.0.saturating_add(inclusive_span / 2))];
    }
    let divisor = i128::try_from(count.saturating_sub(1)).unwrap_or(i128::MAX);
    (0..count)
        .map(|index| {
            let numerator = i128::from(inclusive_span)
                .saturating_mul(i128::try_from(index).unwrap_or(i128::MAX));
            let offset = i64::try_from(numerator / divisor).unwrap_or(inclusive_span);
            TimeCode(range.start.0.saturating_add(offset))
        })
        .collect()
}

pub(super) fn rgba_mean_absolute_difference_basis_points(
    left: &kinewright_core::RgbaImage,
    right: &kinewright_core::RgbaImage,
) -> Option<u16> {
    if left.width != right.width
        || left.height != right.height
        || left.pixels.len() != right.pixels.len()
        || !left.pixels.len().is_multiple_of(4)
    {
        return None;
    }
    let mut difference = 0_u128;
    let mut channels = 0_u128;
    for (left_pixel, right_pixel) in left
        .pixels
        .as_chunks::<4>()
        .0
        .iter()
        .zip(right.pixels.as_chunks::<4>().0.iter())
    {
        for channel in 0..3 {
            difference = difference.saturating_add(u128::from(
                left_pixel[channel].abs_diff(right_pixel[channel]),
            ));
            channels = channels.saturating_add(1);
        }
    }
    if channels == 0 {
        return None;
    }
    let denominator = channels.saturating_mul(u128::from(u8::MAX));
    let rounded = difference
        .saturating_mul(10_000)
        .saturating_add(denominator / 2)
        / denominator;
    Some(u16::try_from(rounded).unwrap_or(10_000).min(10_000))
}

pub(super) fn compose_contact_sheet(
    images: &[kinewright_core::RgbaImage],
) -> Result<kinewright_core::RgbaImage, McpError> {
    let cell_width = images.iter().map(|image| image.width).max().unwrap_or(1);
    let cell_height = images.iter().map(|image| image.height).max().unwrap_or(1);
    let count = u32::try_from(images.len()).unwrap_or(u32::MAX).max(1);
    let columns = count.min(STORYBOARD_COLUMNS);
    let rows = count.div_ceil(columns);
    let width = cell_width
        .checked_mul(columns)
        .and_then(|value| {
            value.checked_add(STORYBOARD_GUTTER.saturating_mul(columns.saturating_sub(1)))
        })
        .ok_or_else(|| McpError::internal_error("storyboard width overflowed", None))?;
    let height = cell_height
        .checked_mul(rows)
        .and_then(|value| {
            value.checked_add(STORYBOARD_GUTTER.saturating_mul(rows.saturating_sub(1)))
        })
        .ok_or_else(|| McpError::internal_error("storyboard height overflowed", None))?;
    let byte_count = usize::try_from(width)
        .ok()
        .and_then(|width| {
            usize::try_from(height)
                .ok()
                .and_then(|height| width.checked_mul(height))
        })
        .and_then(|pixels| pixels.checked_mul(4))
        .ok_or_else(|| McpError::internal_error("storyboard allocation overflowed", None))?;
    let mut pixels = vec![16_u8; byte_count];
    for alpha in pixels.iter_mut().skip(3).step_by(4) {
        *alpha = u8::MAX;
    }
    for (index, image) in images.iter().enumerate() {
        let index = u32::try_from(index).unwrap_or(u32::MAX);
        let column = index % columns;
        let row = index / columns;
        let x = column.saturating_mul(cell_width.saturating_add(STORYBOARD_GUTTER));
        let y = row.saturating_mul(cell_height.saturating_add(STORYBOARD_GUTTER));
        for source_y in 0..image.height {
            let source_start = usize::try_from(source_y)
                .unwrap_or(usize::MAX)
                .saturating_mul(usize::try_from(image.width).unwrap_or(usize::MAX))
                .saturating_mul(4);
            let source_len = usize::try_from(image.width)
                .unwrap_or(usize::MAX)
                .saturating_mul(4);
            let destination_start = usize::try_from(y.saturating_add(source_y))
                .unwrap_or(usize::MAX)
                .saturating_mul(usize::try_from(width).unwrap_or(usize::MAX))
                .saturating_add(usize::try_from(x).unwrap_or(usize::MAX))
                .saturating_mul(4);
            let Some(source) = image
                .pixels
                .get(source_start..source_start.saturating_add(source_len))
            else {
                return Err(McpError::internal_error(
                    "storyboard source image is truncated",
                    None,
                ));
            };
            let Some(destination) =
                pixels.get_mut(destination_start..destination_start.saturating_add(source_len))
            else {
                return Err(McpError::internal_error(
                    "storyboard destination image overflowed",
                    None,
                ));
            };
            destination.copy_from_slice(source);
        }
    }
    Ok(kinewright_core::RgbaImage {
        width,
        height,
        pixels,
    })
}
