//! Subject/region tracking and reframe planning.

use super::mattes::{
    composite_seed_percent, effect_chain, keyframed_transform_note, layer_scale_extremes,
    layer_unit_to_basis_points, matte_track_centre_basis_points, offending_template_scale,
    resolve_layer_transform_at, tracked_box_percent, tracked_centre_layer_unit,
    tracking_seed_outside_composite_result,
};
use super::*;

impl KinewrightMcp {
    #[allow(clippy::too_many_lines)]
    pub(super) fn track_reframe_subject(
        &self,
        args: &TrackReframeArgs,
    ) -> Result<CallToolResult, McpError> {
        let (revision, document) = self.snapshot()?;
        let Some(clip) = document.clip(args.clip_id) else {
            return Ok(error_text(format!("clip {} does not exist", args.clip_id)));
        };
        if !matches!(clip.content, ClipContent::Media) {
            return Ok(error_text("subject reframe tracking requires a media clip"));
        }
        let Some(effect) = clip
            .effects
            .iter()
            .find(|effect| effect.id == args.effect_id)
        else {
            return Ok(error_text(format!(
                "effect {} does not exist on clip {}",
                args.effect_id, args.clip_id
            )));
        };
        if effect.name != "reframe" {
            return Ok(error_text(format!(
                "effect {} is {}; subject tracking requires a reframe effect",
                args.effect_id, effect.name
            )));
        }
        let Some((source_width, source_height)) = document
            .asset(clip.asset)
            .and_then(|asset| asset.resolution)
        else {
            return Ok(error_text(format!(
                "clip {} source resolution is required to plan full tracked-subject containment",
                args.clip_id
            )));
        };
        if source_width == 0 || source_height == 0 {
            return Ok(error_text(format!(
                "clip {} has invalid source resolution {source_width}x{source_height}",
                args.clip_id
            )));
        }
        if !(1..=75).contains(&args.subject_width_percent)
            || !(1..=75).contains(&args.subject_height_percent)
        {
            return Ok(error_text(
                "subject_width_percent and subject_height_percent must each be in 1..=75",
            ));
        }
        let duration = match document.clip_duration(clip) {
            Ok(duration) => duration,
            Err(error) => return Ok(error_text(error.to_string())),
        };
        let start = args.start_local_frame.unwrap_or(TimeCode::ZERO);
        let end = args.end_local_frame.unwrap_or(duration);
        if start < TimeCode::ZERO || end > duration || end <= start {
            return Ok(error_text(format!(
                "tracking range {start}..{end} is outside clip-local range 0..{duration}"
            )));
        }
        let step = args.step_frames.unwrap_or(DEFAULT_TRACKING_STEP_FRAMES);
        if !(1..=120).contains(&step) {
            return Ok(error_text("step_frames must be in 1..=120"));
        }
        let sample_frames = tracking_sample_frames(start..end, step);
        if sample_frames.len() > MAX_TRACKING_SAMPLES {
            return Ok(error_text(format!(
                "tracking would render {} samples; increase step_frames to stay at or below {MAX_TRACKING_SAMPLES}",
                sample_frames.len()
            )));
        }
        // The stored focus, in the compositor's own precedence: an explicitly
        // stored `focus_*_basis_points` wins over `focus_*_percent`
        // (`compositor.rs`'s `ReframeFocusXBasisPoints` arm only overwrites the
        // percent-derived focus when the parameter is actually present), and a
        // reframe carrying neither is centred. This tool *writes* basis points,
        // so reading only the percent would seed a re-track of its own output
        // at 50 percent instead of where the camera actually is.
        let stored_focus = |basis_points: &str, percent: &str| -> u8 {
            if let Some(value) = effect.integer_parameter_at(basis_points, start) {
                let rounded = (value.clamp(0, 10_000) + 50) / 100;
                return u8::try_from(rounded).unwrap_or(50);
            }
            effect
                .integer_parameter_at(percent, start)
                .map_or(50, |value| u8::try_from(value.clamp(0, 100)).unwrap_or(50))
        };
        let initial_x = args
            .initial_subject_x_percent
            .unwrap_or_else(|| stored_focus("focus_x_basis_points", "focus_x_percent"));
        let initial_y = args
            .initial_subject_y_percent
            .unwrap_or_else(|| stored_focus("focus_y_basis_points", "focus_y_percent"));
        if initial_x > 100 || initial_y > 100 {
            return Ok(error_text(
                "initial_subject_x_percent and initial_subject_y_percent must be in 0..=100",
            ));
        }
        let focus_bounds = [
            args.minimum_focus_x_percent.unwrap_or(0),
            args.maximum_focus_x_percent.unwrap_or(100),
            args.minimum_focus_y_percent.unwrap_or(0),
            args.maximum_focus_y_percent.unwrap_or(100),
        ];
        if focus_bounds.iter().any(|value| *value > 100)
            || focus_bounds[0] > focus_bounds[1]
            || focus_bounds[2] > focus_bounds[3]
        {
            return Ok(error_text(
                "focus bounds must be ordered percentages in 0..=100",
            ));
        }
        let focus_dead_zone = args
            .focus_dead_zone_percent
            .unwrap_or(DEFAULT_REFRAME_DEAD_ZONE_PERCENT);
        if focus_dead_zone > 25 {
            return Ok(error_text("focus_dead_zone_percent must be in 0..=25"));
        }
        let maximum_focus_step = args
            .maximum_focus_step_percent
            .unwrap_or(DEFAULT_REFRAME_MAXIMUM_STEP_PERCENT);
        if !(1..=25).contains(&maximum_focus_step) {
            return Ok(error_text("maximum_focus_step_percent must be in 1..=25"));
        }
        let search_radius = args
            .search_radius_percent
            .unwrap_or(DEFAULT_TRACKING_SEARCH_RADIUS_PERCENT);
        if !(1..=25).contains(&search_radius) {
            return Ok(error_text("search_radius_percent must be in 1..=25"));
        }
        let max_width = args.max_width.unwrap_or(DEFAULT_TRACKING_WIDTH);
        if !(64..=512).contains(&max_width) {
            return Ok(error_text("max_width must be in 64..=512"));
        }
        // CC5 §5.2: `focus_x/y_basis_points` name the centre of the visible
        // window *inside the layer texture* — `compositor.wgsl` builds
        // `sample_uv` from `reframe_focus_x/y` before the vertex stage places
        // the quad — while the tracker measures the composited thumbnail. Seed
        // the search with the initial focus pushed forward through the layer
        // transform resolved at the first sampled frame, and rescale the
        // subject template by that same scale, exactly as `track_matte_window`
        // does for a window.
        let seed_transform = resolve_layer_transform_at(effect_chain(clip), start);
        let seed_center_percent = match composite_seed_percent(
            seed_transform,
            [f64::from(initial_x) / 100.0, f64::from(initial_y) / 100.0],
        ) {
            Ok(percent) => percent,
            Err(seed) => {
                return Ok(tracking_seed_outside_composite_result(
                    ["initial_subject_x_percent", "initial_subject_y_percent"],
                    args.clip_id,
                    start,
                    seed_transform,
                    &seed,
                    &[],
                ));
            }
        };
        let subject_box_percent = [
            i64::from(args.subject_width_percent),
            i64::from(args.subject_height_percent),
        ];
        let box_percent = [
            tracked_box_percent(subject_box_percent[0], seed_transform.scale),
            tracked_box_percent(subject_box_percent[1], seed_transform.scale),
        ];
        // CC5 §5.2: the template is sized once, at the seed frame's scale, but
        // it must be a legal template at *every* sampled frame, so the gate is
        // applied at the smallest and the largest resolved scale and the
        // refusal names the frame and scale that failed rather than the seed's.
        let scale_extremes = layer_scale_extremes(effect_chain(clip), &sample_frames);
        if let Some((offending, [template_width, template_height])) = scale_extremes
            .and_then(|extremes| offending_template_scale(subject_box_percent, extremes))
        {
            return Ok(error_text(format!(
                "subject_width_percent and subject_height_percent must each be in 1..=75 for tracking; the {}x{} percent subject maps to a {}x{} percent template on the composite at layer scale {} at clip-local frame {}",
                subject_box_percent[0],
                subject_box_percent[1],
                template_width,
                template_height,
                offending.scale,
                offending.frame,
            )));
        }
        let tracked = match self.track_clip_region(&RegionTrackingRequest {
            document: &document,
            clip_id: args.clip_id,
            clip_timeline_start: clip.timeline_start,
            sample_frames: &sample_frames,
            center_percent: seed_center_percent,
            box_percent,
            search_radius_percent: search_radius,
            max_width,
            excluded_effect: args.effect_id,
        }) {
            Ok(tracked) => tracked,
            Err(error) => return Ok(error_text(error)),
        };
        // CC5 §5.2: one transform per observation, resolved at that
        // observation's own frame, so a keyframed scale or offset is converted
        // sample by sample rather than refused.
        let sample_transforms = tracked
            .observations
            .iter()
            .map(|observation| {
                resolve_layer_transform_at(effect_chain(clip), observation.local_frame)
            })
            .collect::<Vec<_>>();
        // The bounds the tracker measured, on the composite, from the same
        // rescaled template it matched with. Pure provenance: nothing plans
        // from these, because the template is sized once at the seed frame's
        // scale and converting it back through a *different* observation's
        // scale would inflate the box by `seed_scale / observation_scale`.
        let composite_samples = tracked
            .observations
            .iter()
            .map(|observation| {
                tracked_subject_bounds(observation, tracked.width, tracked.height, box_percent)
            })
            .collect::<Vec<_>>();
        // Every observation's centre, converted into layer uv with the
        // transform resolved at that observation's own frame.
        let layer_centres = tracked
            .observations
            .iter()
            .zip(&sample_transforms)
            .map(|(observation, transform)| {
                tracked_centre_layer_unit(
                    observation.center,
                    tracked.width,
                    tracked.height,
                    *transform,
                )
            })
            .collect::<Vec<_>>();
        // The reframe crop selects a sub-rectangle of the *layer* texture, so
        // the containment constraint — and the provenance marker that records
        // it — are stated in layer uv too. The box is the converted layer
        // centre bracketed by the *declared* layer subject size, rounded
        // outward and clamped to 0..=10000; it is never routed through the
        // composite template, whose size is pinned to the seed frame's scale.
        let provenance_samples = tracked
            .observations
            .iter()
            .zip(&layer_centres)
            .map(|(observation, centre)| {
                layer_subject_bounds(observation.local_frame, *centre, subject_box_percent)
            })
            .collect::<Vec<_>>();
        let containment = provenance_samples
            .iter()
            .map(|subject| {
                let target_aspect_basis_points = effect
                    .integer_parameter_at("target_aspect_basis_points", subject.at)
                    .ok_or_else(|| {
                        format!(
                            "reframe effect {} has no target_aspect_basis_points at frame {}",
                            args.effect_id, subject.at
                        )
                    })?;
                tracked_subject_focus_constraint(
                    *subject,
                    source_width,
                    source_height,
                    target_aspect_basis_points,
                )
            })
            .collect::<Result<Vec<_>, _>>();
        let containment = match containment {
            Ok(constraints) => constraints,
            Err(error) => return Ok(error_text(error)),
        };
        // CC5 §5.2: every observation is converted *before* the planner sees
        // it — composite pixel as a fraction of the extent, then pulled back
        // into layer uv — so the focus curve is planned, clamped, and written
        // entirely in the space the compositor reads it in.
        let samples = tracked
            .observations
            .iter()
            .zip(&layer_centres)
            .map(|(observation, layer)| SubjectCenterBasisPointSample {
                at: observation.local_frame,
                x_basis_points: layer_unit_to_basis_points(layer[0]),
                y_basis_points: layer_unit_to_basis_points(layer[1]),
                confidence_basis_points: observation.confidence_basis_points,
            })
            .collect::<Vec<_>>();
        let reframe = match plan_subject_reframe_basis_points_with_containment(
            &document,
            SubjectReframeSettings {
                clip: args.clip_id,
                effect: args.effect_id,
                bounds: ReframeFocusBounds {
                    min_x_percent: i64::from(focus_bounds[0]),
                    max_x_percent: i64::from(focus_bounds[1]),
                    min_y_percent: i64::from(focus_bounds[2]),
                    max_y_percent: i64::from(focus_bounds[3]),
                },
                minimum_confidence_basis_points: 0,
                focus_dead_zone_percent: focus_dead_zone,
                maximum_focus_step_percent: maximum_focus_step,
            },
            &samples,
            &containment,
        ) {
            Ok(plan) => plan,
            Err(error) => {
                return Ok(error_text(format!(
                    "full tracked-subject containment could not be planned: {error}"
                )));
            }
        };
        let x_curve = &reframe.focus_x_curve;
        let y_curve = &reframe.focus_y_curve;
        let focus_keyframes = tracked
            .observations
            .iter()
            .zip(&x_curve.keyframes)
            .zip(&y_curve.keyframes)
            .map(|((observation, x), y)| {
                serde_json::json!({
                    "frame": observation.local_frame.0,
                    "x_basis_points": x.value,
                    "y_basis_points": y.value,
                    "confidence": observation.confidence_basis_points,
                })
            })
            .collect::<Vec<_>>();
        let minimum_confidence = tracked
            .observations
            .iter()
            .map(|observation| observation.confidence_basis_points)
            .min()
            .unwrap_or_default();
        let provenance = ReframeSubjectProvenance {
            clip: args.clip_id,
            effect: args.effect_id,
            samples: provenance_samples.clone(),
        };
        let provenance_label = encode_reframe_subject_provenance(&provenance);
        let existing_provenance_marker = document.markers.iter().find_map(|marker| {
            decode_reframe_subject_provenance(&marker.label)
                .ok()
                .flatten()
                .filter(|existing| {
                    existing.clip == args.clip_id && existing.effect == args.effect_id
                })
                .map(|_| marker.id)
        });
        let provenance_operation = if let Some(marker) = existing_provenance_marker {
            Operation::SetMarkerParam {
                marker,
                name: "label".to_owned(),
                value: ParamValue::Text(provenance_label),
            }
        } else {
            let next_marker_id = document
                .markers
                .iter()
                .map(|marker| marker.id.0)
                .max()
                .unwrap_or_default()
                .checked_add(1)
                .map(MarkerId)
                .ok_or_else(|| {
                    McpError::internal_error("marker id space is exhausted".to_owned(), None)
                })?;
            Operation::AddMarker {
                marker: Marker {
                    id: next_marker_id,
                    position: clip.timeline_start,
                    label: provenance_label,
                    color_token: 3,
                },
            }
        };
        // CC5 §5.2 provenance: the raw composite measurement beside the layer
        // value that was actually planned from it, one row per sample.
        let subject_samples = tracked
            .observations
            .iter()
            .zip(&samples)
            .zip(&sample_transforms)
            .zip(&composite_samples)
            .zip(&provenance_samples)
            .map(|((((observation, sample), transform), composite), layer)| {
                serde_json::json!({
                    "local_frame": observation.local_frame.0,
                    "project_frame": observation.project_frame.0,
                    "layer_x_basis_points": sample.x_basis_points,
                    "layer_y_basis_points": sample.y_basis_points,
                    "composite_center_pixel": observation.center,
                    "composite_x_basis_points": matte_track_centre_basis_points(
                        observation.center[0],
                        tracked.width,
                    ),
                    "composite_y_basis_points": matte_track_centre_basis_points(
                        observation.center[1],
                        tracked.height,
                    ),
                    "composite_bounds_basis_points": {
                        "left": composite.left_basis_points,
                        "right": composite.right_basis_points,
                        "top": composite.top_basis_points,
                        "bottom": composite.bottom_basis_points,
                    },
                    // The box containment was planned from and the provenance
                    // marker records: the converted layer centre bracketed by
                    // the declared layer subject size, rounded outward.
                    "layer_bounds_basis_points": {
                        "left": layer.left_basis_points,
                        "right": layer.right_basis_points,
                        "top": layer.top_basis_points,
                        "bottom": layer.bottom_basis_points,
                    },
                    "layer_transform": {
                        "scale": transform.scale,
                        "offset_x": transform.offset_x,
                        "offset_y": transform.offset_y,
                    },
                    "confidence_basis_points": observation.confidence_basis_points,
                })
            })
            .collect::<Vec<_>>();
        let mut operations = reframe.operations;
        operations.push(provenance_operation);
        let plan = match self.prepare_operations(revision, &document, operations) {
            Ok(plan) => plan,
            Err(error) => {
                return Ok(error_text(format!(
                    "tracked reframe keyframes do not fit the current clip: {error}"
                )));
            }
        };
        let structured = serde_json::json!({
            "timeline_revision": revision.0,
            "clip_id": args.clip_id.0,
            "effect_id": args.effect_id.0,
            "range": {"start": start.0, "end": end.0, "step_frames": step},
            "subject_template": {
                "width_percent": args.subject_width_percent,
                "height_percent": args.subject_height_percent,
                "initial_center_percent": {"x": initial_x, "y": initial_y},
                "composite_box_percent": box_percent,
                "composite_seed_center_percent": seed_center_percent,
            },
            // CC5 §5.2: the two spaces and the exact maps between them, stated
            // rather than inferred, mirroring `track_matte_window`.
            "coordinate_space": {
                "measured_on": "composited thumbnail, whose uv is the output frame",
                "written_in": "layer uv, which is where the reframe crop window is centred",
                "thumbnail": {"width": tracked.width, "height": tracked.height},
                "pixel_to_unit": "u_composite = (pixel + 0.5) / extent",
                "composite_to_layer": "u_layer = (u_composite - 0.5) / scale - (offset_x, offset_y) / (2 * scale) + 0.5",
                "unit_to_basis_points": "focus_basis_points = round(u_layer * 10000), clamped to 0..=10000",
                "seed_center_percent": seed_center_percent,
                "box_percent": box_percent,
                "box_percent_rule": "the subject template rescaled by the layer scale: box_percent = round([subject_width_percent, subject_height_percent] * scale) (CC5 §5.2)",
                "per_frame_transform": true,
                "keyframed_transform": keyframed_transform_note(seed_transform.scale, scale_extremes),
                "containment_space": "containment is planned in layer uv: each sample's box is the converted layer centre bracketed by the declared subject_width/height_percent (half extent = percent * 50 basis points), rounded outward — floor on left/top, ceil on right/bottom — and clamped to 0..=10000. The composite template bounds are provenance only and are never converted into the constraint, because that template is sized once at the seed frame's scale. The provenance marker records these layer-space bounds",
                "samples": tracked
                    .observations
                    .iter()
                    .zip(&sample_transforms)
                    .map(|(observation, transform)| serde_json::json!({
                        "local_frame": observation.local_frame.0,
                        "scale": transform.scale,
                        "offset_x": transform.offset_x,
                        "offset_y": transform.offset_y,
                    }))
                    .collect::<Vec<_>>(),
            },
            "subject_samples": subject_samples,
            "focus_bounds_percent": {
                "minimum_x": focus_bounds[0],
                "maximum_x": focus_bounds[1],
                "minimum_y": focus_bounds[2],
                "maximum_y": focus_bounds[3],
            },
            "camera_stabilization": {
                "controller": "offline_lookahead_containment",
                "subject_dead_zone_percent": focus_dead_zone,
                "maximum_step_percent": maximum_focus_step,
                "observation_filter": "three_sample_median",
                "keyframe_interpolation": "linear",
            },
            "minimum_confidence_basis_points": minimum_confidence,
            "focus_keyframes": focus_keyframes,
            "prepared_edit_plan": {
                "plan_id": plan.id,
                "expected_revision": revision,
                "preview": plan.preview,
            },
            "detection_boundary": "tracks the explicitly supplied subject region; no learned person or face detection",
        });
        Ok(success_structured(
            format!(
                "tracked and stabilized reframe effect {} on clip {} across {} samples (minimum confidence {minimum_confidence}/10000) as edit plan {}; review low-confidence spans and the preview, then commit it at timeline revision {revision}",
                args.effect_id,
                args.clip_id,
                tracked.observations.len(),
                plan.id,
            ),
            structured,
        ))
    }

    pub(super) fn track_clip_region(
        &self,
        request: &RegionTrackingRequest<'_>,
    ) -> Result<TrackedRegion, String> {
        let mut isolated = request.document.clone();
        for track in &mut isolated.tracks {
            track
                .clips
                .retain(|candidate| candidate.id == request.clip_id);
            for candidate in &mut track.clips {
                candidate
                    .effects
                    .retain(|effect| effect.id != request.excluded_effect);
            }
        }
        isolated.tracks.retain(|track| !track.clips.is_empty());
        let isolated = Arc::new(isolated);
        let project_frame = |local: TimeCode| {
            request
                .clip_timeline_start
                .checked_add(local)
                .ok_or_else(|| "tracking frame overflowed".to_owned())
        };
        let Some(first_local) = request.sample_frames.first().copied() else {
            return Err("tracking requires at least one sample".to_owned());
        };
        let first_project = project_frame(first_local)?;
        let mut previous = self
            .analysis
            .thumbnail_for_document(Arc::clone(&isolated), first_project, request.max_width)
            .map_err(|error| error.to_string())?;
        let width = previous.width;
        let height = previous.height;
        let half_size = tracking_half_size(&previous, request.box_percent);
        let mut center = clamp_tracking_center(
            &previous,
            [
                percent_to_pixel(request.center_percent[0], width),
                percent_to_pixel(request.center_percent[1], height),
            ],
            half_size,
        );
        let mut observations = vec![TrackingObservation {
            local_frame: first_local,
            project_frame: first_project,
            center,
            confidence_basis_points: 10_000,
        }];

        for local_frame in request.sample_frames.iter().copied().skip(1) {
            let project_frame = project_frame(local_frame)?;
            let current = self
                .analysis
                .thumbnail_for_document(Arc::clone(&isolated), project_frame, request.max_width)
                .map_err(|error| error.to_string())?;
            if current.width != width || current.height != height {
                return Err("tracking compositor resolution changed between samples".to_owned());
            }
            let tracked = track_region(
                &previous,
                &current,
                center,
                half_size,
                request.search_radius_percent,
            );
            center = tracked.center;
            observations.push(TrackingObservation {
                local_frame,
                project_frame,
                center,
                confidence_basis_points: tracked.confidence_basis_points,
            });
            previous = current;
        }

        Ok(TrackedRegion {
            observations,
            width,
            height,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct TrackingObservation {
    pub(super) local_frame: TimeCode,
    pub(super) project_frame: TimeCode,
    pub(super) center: [u32; 2],
    pub(super) confidence_basis_points: u16,
}

/// Conservative source-normalized bounds for one tracked subject sample.
///
/// Coordinates use basis points so the evaluator can distinguish an actual
/// camera follow from an integer-percent approximation without making the
/// reframe effect itself carry non-rendering parameters.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct TrackedSubjectBounds {
    pub at: TimeCode,
    pub left_basis_points: u16,
    pub right_basis_points: u16,
    pub top_basis_points: u16,
    pub bottom_basis_points: u16,
}

/// Compact, document-persisted tracking evidence associated with one reframe.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ReframeSubjectProvenance {
    pub clip: ClipId,
    pub effect: EffectId,
    pub samples: Vec<TrackedSubjectBounds>,
}

pub(super) struct RegionTrackingRequest<'a> {
    pub(super) document: &'a Document,
    pub(super) clip_id: ClipId,
    pub(super) clip_timeline_start: TimeCode,
    pub(super) sample_frames: &'a [TimeCode],
    pub(super) center_percent: [u8; 2],
    pub(super) box_percent: [i64; 2],
    pub(super) search_radius_percent: u8,
    pub(super) max_width: u32,
    /// CC5 §5.2: exactly one effect id, not every effect sharing a name.
    /// This narrows the exclusion from *every effect with that name* to the one
    /// node being tracked, so a clip carrying two masks keeps the second mask's
    /// alpha in the tracking thumbnails — which is the correct behaviour.
    pub(super) excluded_effect: EffectId,
}

pub(super) struct TrackedRegion {
    pub(super) observations: Vec<TrackingObservation>,
    pub(super) width: u32,
    pub(super) height: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct TrackingMatch {
    pub(super) center: [u32; 2],
    pub(super) confidence_basis_points: u16,
}

pub(crate) fn encode_reframe_subject_provenance(provenance: &ReframeSubjectProvenance) -> String {
    let sample_count = u16::try_from(provenance.samples.len()).unwrap_or(u16::MAX);
    let mut bytes = Vec::with_capacity(
        REFRAME_SUBJECT_PROVENANCE_HEADER_BYTES
            .saturating_add(usize::from(sample_count) * REFRAME_SUBJECT_PROVENANCE_SAMPLE_BYTES),
    );
    bytes.extend_from_slice(&provenance.clip.0.to_le_bytes());
    bytes.extend_from_slice(&provenance.effect.0.to_le_bytes());
    bytes.extend_from_slice(&sample_count.to_le_bytes());
    for sample in provenance.samples.iter().take(usize::from(sample_count)) {
        bytes.extend_from_slice(&sample.at.0.to_le_bytes());
        bytes.extend_from_slice(&sample.left_basis_points.to_le_bytes());
        bytes.extend_from_slice(&sample.right_basis_points.to_le_bytes());
        bytes.extend_from_slice(&sample.top_basis_points.to_le_bytes());
        bytes.extend_from_slice(&sample.bottom_basis_points.to_le_bytes());
    }
    format!(
        "{REFRAME_SUBJECT_PROVENANCE_PREFIX}{}",
        URL_SAFE_NO_PAD.encode(bytes)
    )
}

/// Decode one opaque marker-label tracking sidecar.
///
/// Non-provenance labels return `Ok(None)` so ordinary user markers stay
/// entirely outside this contract. A matching prefix with malformed data is
/// intentionally an error: silently ignoring corrupted tracking evidence
/// would let a static or wrong-direction reframe pass evaluation.
pub(crate) fn decode_reframe_subject_provenance(
    label: &str,
) -> Result<Option<ReframeSubjectProvenance>, String> {
    let Some(encoded) = label.strip_prefix(REFRAME_SUBJECT_PROVENANCE_PREFIX) else {
        return Ok(None);
    };
    let bytes = URL_SAFE_NO_PAD
        .decode(encoded)
        .map_err(|error| format!("invalid base64: {error}"))?;
    if bytes.len() < REFRAME_SUBJECT_PROVENANCE_HEADER_BYTES {
        return Err("missing provenance header".to_owned());
    }
    let decode_id = |offset: usize| {
        let slice = bytes
            .get(offset..offset.saturating_add(8))
            .ok_or_else(|| "truncated provenance header".to_owned())?;
        let array: [u8; 8] = slice
            .try_into()
            .map_err(|_| "invalid provenance header width".to_owned())?;
        Ok::<u64, String>(u64::from_le_bytes(array))
    };
    let read_u16 = |offset: usize| {
        let slice = bytes
            .get(offset..offset.saturating_add(2))
            .ok_or_else(|| "truncated provenance sample".to_owned())?;
        let array: [u8; 2] = slice
            .try_into()
            .map_err(|_| "invalid provenance sample width".to_owned())?;
        Ok::<u16, String>(u16::from_le_bytes(array))
    };
    let decode_frame = |offset: usize| {
        let slice = bytes
            .get(offset..offset.saturating_add(8))
            .ok_or_else(|| "truncated provenance sample".to_owned())?;
        let array: [u8; 8] = slice
            .try_into()
            .map_err(|_| "invalid provenance sample width".to_owned())?;
        Ok::<i64, String>(i64::from_le_bytes(array))
    };
    let clip = ClipId(decode_id(0)?);
    let effect = EffectId(decode_id(8)?);
    let sample_count = usize::from(read_u16(16)?);
    if sample_count > MAX_TRACKING_SAMPLES {
        return Err(format!(
            "contains {sample_count} samples, above the {MAX_TRACKING_SAMPLES} sample limit"
        ));
    }
    let expected_length = REFRAME_SUBJECT_PROVENANCE_HEADER_BYTES
        .saturating_add(sample_count.saturating_mul(REFRAME_SUBJECT_PROVENANCE_SAMPLE_BYTES));
    if bytes.len() != expected_length {
        return Err(format!(
            "expected {expected_length} bytes for {sample_count} samples, found {}",
            bytes.len()
        ));
    }
    if sample_count == 0 {
        return Err("contains no tracked subject samples".to_owned());
    }
    let mut samples = Vec::with_capacity(sample_count);
    for index in 0..sample_count {
        let offset = REFRAME_SUBJECT_PROVENANCE_HEADER_BYTES
            .saturating_add(index.saturating_mul(REFRAME_SUBJECT_PROVENANCE_SAMPLE_BYTES));
        let at = TimeCode(decode_frame(offset)?);
        let left_basis_points = read_u16(offset + 8)?;
        let right_basis_points = read_u16(offset + 10)?;
        let top_basis_points = read_u16(offset + 12)?;
        let bottom_basis_points = read_u16(offset + 14)?;
        if at < TimeCode::ZERO
            || left_basis_points > right_basis_points
            || top_basis_points > bottom_basis_points
            || right_basis_points > 10_000
            || bottom_basis_points > 10_000
        {
            return Err(format!("sample {index} has invalid bounds"));
        }
        if samples
            .last()
            .is_some_and(|previous: &TrackedSubjectBounds| at <= previous.at)
        {
            return Err(format!("sample {index} is not strictly ordered"));
        }
        samples.push(TrackedSubjectBounds {
            at,
            left_basis_points,
            right_basis_points,
            top_basis_points,
            bottom_basis_points,
        });
    }
    Ok(Some(ReframeSubjectProvenance {
        clip,
        effect,
        samples,
    }))
}

// ---------------------------------------------------------------------------
// CC5 §5.2 — matte window tracking
// ---------------------------------------------------------------------------

/// One tracked subject box, stated directly in layer uv (CC5 §5.2).
///
/// The reframe crop is a sub-rectangle of the *layer* texture, so the
/// containment constraint — and the provenance marker that records it — must be
/// stated in layer basis points. The box is built from the converted layer
/// centre and the **declared** layer subject size, never by converting the
/// composite template's own bounds: the template is sized once, at the seed
/// frame's scale, so converting it back through a *different* observation's
/// scale would inflate the box by `seed_scale / observation_scale`.
///
/// `subject_percent` is a full width/height in layer percent, so each half
/// extent is `percent · 50` basis points. Edges round **outward** — floor on
/// left/top, ceil on right/bottom — so the recorded box is never smaller than
/// the measured one and `eval.rs`'s zero-tolerance containment check stays
/// conservative. The result is clamped to `0..=10000` because the crop can only
/// sample layer uv `0..1`.
pub(super) fn layer_subject_bounds(
    at: TimeCode,
    layer_centre: [f64; 2],
    subject_percent: [i64; 2],
) -> TrackedSubjectBounds {
    let edge = |centre: f64, percent: i64, upper: bool| -> u16 {
        #[allow(clippy::cast_precision_loss)]
        let half = percent as f64 * 50.0;
        // A basis point is the finest unit these parameters carry, so an edge
        // that is analytically integral is snapped onto the grid before the
        // outward rounding. Without it the last bits of the affine conversion
        // would inflate every exact box by a whole basis point on the ceil
        // side; 1e-6 bp is a thousand times the worst-case error at this
        // magnitude and a millionth of the finest real unit.
        let snap = |value: f64| {
            let nearest = value.round();
            if (value - nearest).abs() < 1e-6 {
                nearest
            } else {
                value
            }
        };
        let value = snap(centre * 10_000.0);
        let value = if upper {
            (value + half).ceil()
        } else {
            (value - half).floor()
        };
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let clamped = value.clamp(0.0, 10_000.0) as u16;
        clamped
    };
    TrackedSubjectBounds {
        at,
        left_basis_points: edge(layer_centre[0], subject_percent[0], false),
        right_basis_points: edge(layer_centre[0], subject_percent[0], true),
        top_basis_points: edge(layer_centre[1], subject_percent[1], false),
        bottom_basis_points: edge(layer_centre[1], subject_percent[1], true),
    }
}

/// The template's own bounds on the composited thumbnail, as **provenance**.
///
/// Nothing plans from these: the containment constraint is built by
/// [`layer_subject_bounds`] from the declared layer subject size. They are
/// published so a reader can see what the tracker actually matched.
///
/// The conversion is the same fraction-of-the-extent convention
/// [`tracker_pixel_to_composite_unit`] uses everywhere else in this path — the
/// matched pixel *covers* `[pixel, pixel + 1) / extent` — rounded outward, so
/// this tool never mixes the lattice (`extent − 1`) convention with the
/// fractional one.
fn tracked_subject_bounds(
    observation: &TrackingObservation,
    width: u32,
    height: u32,
    box_percent: [i64; 2],
) -> TrackedSubjectBounds {
    let half_size = [
        tracking_half_extent(width, box_percent[0]),
        tracking_half_extent(height, box_percent[1]),
    ];
    let left = observation.center[0].saturating_sub(half_size[0]);
    let right = observation.center[0]
        .saturating_add(half_size[0])
        .min(width.saturating_sub(1));
    let top = observation.center[1].saturating_sub(half_size[1]);
    let bottom = observation.center[1]
        .saturating_add(half_size[1])
        .min(height.saturating_sub(1));
    TrackedSubjectBounds {
        at: observation.local_frame,
        left_basis_points: composite_edge_basis_points(left, width, false),
        right_basis_points: composite_edge_basis_points(right, width, true),
        top_basis_points: composite_edge_basis_points(top, height, false),
        bottom_basis_points: composite_edge_basis_points(bottom, height, true),
    }
}

/// One thumbnail pixel edge as a fraction of the extent, rounded outward.
///
/// `upper` names the pixel's far edge, `(pixel + 1) / extent`, so a one-pixel
/// box is one pixel wide rather than zero.
fn composite_edge_basis_points(pixel: u32, extent: u32, upper: bool) -> u16 {
    let extent = u64::from(extent.max(1));
    let numerator = u64::from(pixel)
        .saturating_add(u64::from(upper))
        .saturating_mul(10_000);
    let value = if upper {
        numerator
            .saturating_add(extent.saturating_sub(1))
            .saturating_div(extent)
    } else {
        numerator.saturating_div(extent)
    };
    u16::try_from(value.min(10_000)).unwrap_or(10_000)
}

pub(super) fn tracking_sample_frames(range: std::ops::Range<TimeCode>, step: i64) -> Vec<TimeCode> {
    let Some(last) = range.end.0.checked_sub(1) else {
        return Vec::new();
    };
    if last < range.start.0 {
        return Vec::new();
    }
    if last == range.start.0 {
        return vec![range.start];
    }

    // Treat `step` as the requested maximum spacing, then distribute the
    // samples across the whole visible span. Appending `last` after stepping
    // leaves a one-frame tail whenever the span is not divisible by `step`.
    // Evenly distributing ceil(span / step) intervals keeps every gap within
    // one frame of its neighbours and makes the final interval ordinary.
    let span = i128::from(last) - i128::from(range.start.0);
    let requested_step = i128::from(step.max(1));
    let interval_count = usize::try_from((span + requested_step - 1) / requested_step)
        .unwrap_or(usize::MAX)
        .max(1);
    let interval_count_i128 = i128::try_from(interval_count).unwrap_or(i128::MAX);
    let mut frames = Vec::with_capacity(interval_count.saturating_add(1));
    for index in 0..=interval_count {
        let index_i128 = i128::try_from(index).unwrap_or(i128::MAX);
        let offset = span.saturating_mul(index_i128) / interval_count_i128;
        let frame = if index == interval_count {
            last
        } else {
            i64::try_from(i128::from(range.start.0).saturating_add(offset)).unwrap_or(last)
        };
        frames.push(TimeCode(frame));
    }
    frames
}

fn tracking_half_size(image: &kinewright_core::RgbaImage, box_percent: [i64; 2]) -> [u32; 2] {
    [
        tracking_half_extent(image.width, box_percent[0]),
        tracking_half_extent(image.height, box_percent[1]),
    ]
}

fn tracking_half_extent(extent: u32, percent: i64) -> u32 {
    let percent = u32::try_from(percent).unwrap_or_default();
    extent
        .saturating_mul(percent)
        .div_ceil(200)
        .max(1)
        .min(extent.saturating_sub(1) / 2)
}

fn percent_to_pixel(percent: u8, extent: u32) -> u32 {
    u32::from(percent)
        .saturating_mul(extent.saturating_sub(1))
        .saturating_add(50)
        / 100
}

pub(super) fn tracked_subject_focus_constraint(
    subject: TrackedSubjectBounds,
    source_width: u32,
    source_height: u32,
    target_aspect_basis_points: i64,
) -> Result<SubjectFocusBasisPointConstraint, String> {
    if source_width == 0 || source_height == 0 {
        return Err(format!(
            "source resolution must be positive, found {source_width}x{source_height}"
        ));
    }
    if target_aspect_basis_points <= 0 {
        return Err(format!(
            "target_aspect_basis_points must be positive, found {target_aspect_basis_points}"
        ));
    }

    let source_width = i128::from(source_width);
    let source_height = i128::from(source_height);
    let target_aspect = i128::from(target_aspect_basis_points);
    let source_is_wider =
        source_width.saturating_mul(10_000) > source_height.saturating_mul(target_aspect);
    let source_is_taller =
        source_width.saturating_mul(10_000) < source_height.saturating_mul(target_aspect);
    let (visible_width, visible_height) = if source_is_wider {
        (
            i64::try_from(ceil_positive_ratio(
                target_aspect.saturating_mul(source_height),
                source_width,
            ))
            .unwrap_or(10_000)
            .clamp(1, 10_000),
            10_000,
        )
    } else if source_is_taller {
        (
            10_000,
            i64::try_from(ceil_positive_ratio(
                source_width.saturating_mul(100_000_000),
                source_height.saturating_mul(target_aspect),
            ))
            .unwrap_or(10_000)
            .clamp(1, 10_000),
        )
    } else {
        (10_000, 10_000)
    };
    let (minimum_x, maximum_x) = focus_interval_for_subject_axis(
        i64::from(subject.left_basis_points),
        i64::from(subject.right_basis_points),
        visible_width,
    )
    .ok_or_else(|| {
        format!(
            "tracked subject at frame {} is wider than the delivery crop",
            subject.at
        )
    })?;
    let (minimum_y, maximum_y) = focus_interval_for_subject_axis(
        i64::from(subject.top_basis_points),
        i64::from(subject.bottom_basis_points),
        visible_height,
    )
    .ok_or_else(|| {
        format!(
            "tracked subject at frame {} is taller than the delivery crop",
            subject.at
        )
    })?;

    Ok(SubjectFocusBasisPointConstraint {
        at: subject.at,
        min_x_basis_points: minimum_x,
        max_x_basis_points: maximum_x,
        min_y_basis_points: minimum_y,
        max_y_basis_points: maximum_y,
    })
}

fn ceil_positive_ratio(numerator: i128, denominator: i128) -> i128 {
    numerator
        .saturating_add(denominator.saturating_sub(1))
        .checked_div(denominator.max(1))
        .unwrap_or_default()
}

/// Invert the compositor's clamped crop-axis transform.
///
/// At either frame edge, many focus values produce the same clamped crop. The
/// returned interval retains those plateaus instead of forcing the virtual
/// camera toward an arbitrary centre value.
pub(super) fn focus_interval_for_subject_axis(
    subject_minimum: i64,
    subject_maximum: i64,
    visible_basis_points: i64,
) -> Option<(i64, i64)> {
    let visible = visible_basis_points.clamp(1, 10_000);
    if subject_minimum < 0
        || subject_maximum > 10_000
        || subject_minimum > subject_maximum
        || subject_maximum.saturating_sub(subject_minimum) > visible
    {
        return None;
    }
    let maximum_crop_start = 10_000_i64.saturating_sub(visible);
    let minimum_crop_start = subject_maximum.saturating_sub(visible).max(0);
    let maximum_allowed_crop_start = subject_minimum.min(maximum_crop_start);
    if minimum_crop_start > maximum_allowed_crop_start {
        return None;
    }
    let half_visible = visible / 2;
    let minimum_focus = if minimum_crop_start == 0 {
        0
    } else {
        minimum_crop_start.saturating_add(half_visible)
    };
    let maximum_focus = if maximum_allowed_crop_start == maximum_crop_start {
        10_000
    } else {
        maximum_allowed_crop_start.saturating_add(half_visible)
    };
    Some((minimum_focus, maximum_focus))
}

fn clamp_tracking_center(
    image: &kinewright_core::RgbaImage,
    center: [u32; 2],
    half_size: [u32; 2],
) -> [u32; 2] {
    let clamp = |value: u32, extent: u32, half: u32| {
        value.clamp(half, extent.saturating_sub(half).saturating_sub(1))
    };
    [
        clamp(center[0], image.width, half_size[0]),
        clamp(center[1], image.height, half_size[1]),
    ]
}

pub(super) fn track_region(
    previous: &kinewright_core::RgbaImage,
    current: &kinewright_core::RgbaImage,
    previous_center: [u32; 2],
    half_size: [u32; 2],
    search_radius_percent: u8,
) -> TrackingMatch {
    let radius = [
        previous
            .width
            .saturating_mul(u32::from(search_radius_percent))
            .div_ceil(100)
            .max(1),
        previous
            .height
            .saturating_mul(u32::from(search_radius_percent))
            .div_ceil(100)
            .max(1),
    ];
    let minimum = [
        previous_center[0]
            .saturating_sub(radius[0])
            .max(half_size[0]),
        previous_center[1]
            .saturating_sub(radius[1])
            .max(half_size[1]),
    ];
    let maximum = [
        previous_center[0]
            .saturating_add(radius[0])
            .min(current.width.saturating_sub(half_size[0]).saturating_sub(1)),
        previous_center[1].saturating_add(radius[1]).min(
            current
                .height
                .saturating_sub(half_size[1])
                .saturating_sub(1),
        ),
    ];
    let coarse_step = radius[0].max(radius[1]).div_ceil(8).max(1);
    let sample_step = half_size[0]
        .saturating_mul(2)
        .saturating_add(1)
        .max(half_size[1].saturating_mul(2).saturating_add(1))
        .div_ceil(24)
        .max(1);
    let mut best = (
        u64::MAX,
        u32::MAX,
        previous_center[1],
        previous_center[0],
        1_u64,
    );
    for y in candidate_axis(minimum[1], maximum[1], coarse_step) {
        for x in candidate_axis(minimum[0], maximum[0], coarse_step) {
            best = best.min(tracking_candidate(
                previous,
                current,
                previous_center,
                [x, y],
                half_size,
                sample_step,
            ));
        }
    }
    let coarse_center = [best.3, best.2];
    let refine_minimum = [
        coarse_center[0].saturating_sub(coarse_step).max(minimum[0]),
        coarse_center[1].saturating_sub(coarse_step).max(minimum[1]),
    ];
    let refine_maximum = [
        coarse_center[0].saturating_add(coarse_step).min(maximum[0]),
        coarse_center[1].saturating_add(coarse_step).min(maximum[1]),
    ];
    for y in refine_minimum[1]..=refine_maximum[1] {
        for x in refine_minimum[0]..=refine_maximum[0] {
            best = best.min(tracking_candidate(
                previous,
                current,
                previous_center,
                [x, y],
                half_size,
                sample_step,
            ));
        }
    }
    let maximum_sad = best.4.saturating_mul(3 * u64::from(u8::MAX)).max(1);
    let error_basis_points = best.0.saturating_mul(10_000) / maximum_sad;
    TrackingMatch {
        center: [best.3, best.2],
        confidence_basis_points: u16::try_from(
            10_000_u64.saturating_sub(error_basis_points.min(10_000)),
        )
        .unwrap_or_default(),
    }
}

fn tracking_candidate(
    previous: &kinewright_core::RgbaImage,
    current: &kinewright_core::RgbaImage,
    previous_center: [u32; 2],
    candidate_center: [u32; 2],
    half_size: [u32; 2],
    sample_step: u32,
) -> (u64, u32, u32, u32, u64) {
    let (score, samples) = region_sad(
        previous,
        current,
        previous_center,
        candidate_center,
        half_size,
        sample_step,
    );
    let distance = candidate_center[0]
        .abs_diff(previous_center[0])
        .saturating_add(candidate_center[1].abs_diff(previous_center[1]));
    (
        score,
        distance,
        candidate_center[1],
        candidate_center[0],
        samples,
    )
}

fn candidate_axis(minimum: u32, maximum: u32, step: u32) -> Vec<u32> {
    let mut values = (minimum..=maximum)
        .step_by(usize::try_from(step).unwrap_or(1).max(1))
        .collect::<Vec<_>>();
    if values.last() != Some(&maximum) {
        values.push(maximum);
    }
    values
}

fn region_sad(
    previous: &kinewright_core::RgbaImage,
    current: &kinewright_core::RgbaImage,
    previous_center: [u32; 2],
    candidate_center: [u32; 2],
    half_size: [u32; 2],
    sample_step: u32,
) -> (u64, u64) {
    let step = usize::try_from(sample_step).unwrap_or(1).max(1);
    let mut sad = 0_u64;
    let mut samples = 0_u64;
    for offset_y in (0..=half_size[1].saturating_mul(2)).step_by(step) {
        for offset_x in (0..=half_size[0].saturating_mul(2)).step_by(step) {
            let previous_x = previous_center[0]
                .saturating_sub(half_size[0])
                .saturating_add(offset_x);
            let previous_y = previous_center[1]
                .saturating_sub(half_size[1])
                .saturating_add(offset_y);
            let current_x = candidate_center[0]
                .saturating_sub(half_size[0])
                .saturating_add(offset_x);
            let current_y = candidate_center[1]
                .saturating_sub(half_size[1])
                .saturating_add(offset_y);
            let previous_index = usize::try_from(
                previous_y
                    .saturating_mul(previous.width)
                    .saturating_add(previous_x)
                    .saturating_mul(4),
            )
            .unwrap_or_default();
            let current_index = usize::try_from(
                current_y
                    .saturating_mul(current.width)
                    .saturating_add(current_x)
                    .saturating_mul(4),
            )
            .unwrap_or_default();
            for channel in 0..3 {
                sad = sad.saturating_add(u64::from(
                    previous.pixels[previous_index + channel]
                        .abs_diff(current.pixels[current_index + channel]),
                ));
            }
            samples = samples.saturating_add(1);
        }
    }
    (sad, samples)
}
