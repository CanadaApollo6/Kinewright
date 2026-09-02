//! CC5 matte inspectors and trackers, plus the composite <-> layer geometry shared with tracking.

use super::tracking::{RegionTrackingRequest, tracking_sample_frames};
use super::*;

impl KinewrightMcp {
    #[allow(clippy::too_many_lines)]
    pub(super) fn track_mask_region(
        &self,
        args: &TrackMaskArgs,
    ) -> Result<CallToolResult, McpError> {
        let (revision, document) = self.snapshot()?;
        let Some(clip) = document.clip(args.clip_id) else {
            return Ok(error_text(format!("clip {} does not exist", args.clip_id)));
        };
        if !matches!(clip.content, ClipContent::Media) {
            return Ok(error_text("mask tracking requires a media clip"));
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
        if effect.name != "mask" {
            return Ok(error_text(format!(
                "effect {} is {}; mask tracking requires a mask effect",
                args.effect_id, effect.name
            )));
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
        let parameter =
            |name: &str, neutral: i64| effect.integer_parameter_at(name, start).unwrap_or(neutral);
        // CC5 §5.2: `mask_center_x/y_percent` are evaluated at the fragment's
        // *layer* uv (`compositor.wgsl` reads `value / 100` of `input.uv`),
        // which the vertex stage's `scale`/`offset` placement has not yet
        // touched, while the tracker measures the *composited* thumbnail. Seed
        // the search with the stored centre pushed forward through the layer
        // transform resolved at the first sampled frame, and rescale the
        // template by that same scale — exactly as `track_matte_window` does.
        let seed_transform = resolve_layer_transform_at(effect_chain(clip), start);
        let stored_center_percent = [
            parameter("center_x_percent", 50).clamp(0, 100),
            parameter("center_y_percent", 50).clamp(0, 100),
        ];
        #[allow(clippy::cast_precision_loss)]
        let seed_layer = [
            stored_center_percent[0] as f64 / 100.0,
            stored_center_percent[1] as f64 / 100.0,
        ];
        let center_percent = match composite_seed_percent(seed_transform, seed_layer) {
            Ok(percent) => percent,
            Err(seed) => {
                return Ok(tracking_seed_outside_composite_result(
                    ["center_x_percent", "center_y_percent"],
                    args.clip_id,
                    start,
                    seed_transform,
                    &seed,
                    &[],
                ));
            }
        };
        let stored_box_percent = [
            parameter("width_percent", 100),
            parameter("height_percent", 100),
        ];
        let box_percent = [
            tracked_box_percent(stored_box_percent[0], seed_transform.scale),
            tracked_box_percent(stored_box_percent[1], seed_transform.scale),
        ];
        // CC5 §5.2: the template is sized once, at the seed frame's scale, but
        // it must be a legal template at *every* sampled frame. `tracked_box_percent`
        // is monotone in the scale, so testing the smallest and the largest
        // resolved scale tests the whole range — and the refusal names the
        // frame and the scale that failed, not the seed's.
        let scale_extremes = layer_scale_extremes(effect_chain(clip), &sample_frames);
        if let Some((offending, [template_width, template_height])) = scale_extremes
            .and_then(|extremes| offending_template_scale(stored_box_percent, extremes))
        {
            let mut message = "mask width_percent and height_percent must each be in 1..=75 for tracking; set a bounded subject region first".to_owned();
            if (offending.scale - 1.0).abs() > f64::EPSILON {
                use std::fmt::Write as _;
                let _ = write!(
                    message,
                    " (the stored {}x{} percent region maps to a {}x{} percent template on the composite at layer scale {} at clip-local frame {})",
                    stored_box_percent[0],
                    stored_box_percent[1],
                    template_width,
                    template_height,
                    offending.scale,
                    offending.frame,
                );
            }
            return Ok(error_text(message));
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

        let tracked = match self.track_clip_region(&RegionTrackingRequest {
            document: &document,
            clip_id: args.clip_id,
            clip_timeline_start: clip.timeline_start,
            sample_frames: &sample_frames,
            center_percent,
            box_percent,
            search_radius_percent: search_radius,
            max_width,
            excluded_effect: args.effect_id,
        }) {
            Ok(tracked) => tracked,
            Err(error) => return Ok(error_text(error)),
        };
        let observations = tracked.observations;
        // CC5 §5.2: the transform is resolved at *each* observation's own
        // frame, so a keyframed scale or offset is converted sample by sample
        // rather than refused. Every written value is the composite centre
        // measured as a fraction of the extent and pulled back into layer uv.
        let converted = observations
            .iter()
            .map(|observation| {
                let transform =
                    resolve_layer_transform_at(effect_chain(clip), observation.local_frame);
                let layer = tracked_centre_layer_unit(
                    observation.center,
                    tracked.width,
                    tracked.height,
                    transform,
                );
                (
                    transform,
                    [
                        layer_unit_to_percent(layer[0]),
                        layer_unit_to_percent(layer[1]),
                    ],
                )
            })
            .collect::<Vec<_>>();

        let curve_for = |axis: usize| AutomationCurve {
            keyframes: observations
                .iter()
                .zip(&converted)
                .map(|(observation, (_, layer))| Keyframe {
                    at: observation.local_frame,
                    value: layer[axis],
                    interpolation: KeyframeInterpolation::Linear,
                })
                .collect(),
        };
        let x_curve = curve_for(0);
        let y_curve = curve_for(1);
        let operations = vec![
            Operation::SetEffectKeyframes {
                clip: args.clip_id,
                effect: args.effect_id,
                name: "center_x_percent".to_owned(),
                curve: x_curve.clone(),
            },
            Operation::SetEffectKeyframes {
                clip: args.clip_id,
                effect: args.effect_id,
                name: "center_y_percent".to_owned(),
                curve: y_curve.clone(),
            },
        ];
        let observations_json = observations
            .iter()
            .zip(&converted)
            .map(|(observation, (_, layer))| {
                serde_json::json!({
                    "local_frame": observation.local_frame.0,
                    "project_frame": observation.project_frame.0,
                    // The values the plan writes, in the layer uv the mask is
                    // evaluated in.
                    "center_x_percent": layer[0],
                    "center_y_percent": layer[1],
                    "layer_center_x_percent": layer[0],
                    "layer_center_y_percent": layer[1],
                    // Provenance: what the tracker actually measured, on the
                    // composited thumbnail, in its own raster — read with the
                    // *same* fraction-of-the-extent convention this response's
                    // `coordinate_space.pixel_to_unit` publishes and the layer
                    // values above are converted from, so applying the published
                    // `composite_to_layer` map to these numbers reproduces
                    // `layer_center_*_percent`. The `extent − 1` lattice would
                    // silently disagree with the stated map.
                    "composite_center_pixel": observation.center,
                    "composite_center_x_percent": layer_unit_to_percent(
                        tracker_pixel_to_composite_unit(observation.center[0], tracked.width),
                    ),
                    "composite_center_y_percent": layer_unit_to_percent(
                        tracker_pixel_to_composite_unit(observation.center[1], tracked.height),
                    ),
                    "confidence_basis_points": observation.confidence_basis_points,
                })
            })
            .collect::<Vec<_>>();
        let plan = match self.prepare_operations(revision, &document, operations) {
            Ok(plan) => plan,
            Err(error) => {
                return Ok(error_text(format!(
                    "tracked mask keyframes do not fit the current clip: {error}"
                )));
            }
        };
        let structured = serde_json::json!({
            "timeline_revision": revision.0,
            "clip_id": args.clip_id.0,
            "effect_id": args.effect_id.0,
            "range": {"start": start.0, "end": end.0, "step_frames": step},
            "observations": observations_json,
            "curves": {
                "center_x_percent": x_curve,
                "center_y_percent": y_curve,
            },
            // CC5 §5.2: the two spaces and the exact maps between them, stated
            // rather than inferred, mirroring `track_matte_window`.
            "coordinate_space": {
                "measured_on": "composited thumbnail, whose uv is the output frame",
                "written_in": "layer uv, which is where the mask is evaluated",
                "thumbnail": {"width": tracked.width, "height": tracked.height},
                "pixel_to_unit": "u_composite = (pixel + 0.5) / extent",
                "composite_to_layer": "u_layer = (u_composite - 0.5) / scale - (offset_x, offset_y) / (2 * scale) + 0.5",
                "unit_to_percent": "center_percent = round(u_layer * 100), clamped to 0..=100",
                "seed_center_percent": center_percent,
                "box_percent": box_percent,
                "box_percent_rule": "the stored region rescaled by the layer scale: box_percent = round([width_percent, height_percent] * scale) (CC5 §5.2)",
                "per_frame_transform": true,
                "keyframed_transform": keyframed_transform_note(seed_transform.scale, scale_extremes),
                "samples": observations
                    .iter()
                    .zip(&converted)
                    .map(|(observation, (transform, _))| serde_json::json!({
                        "local_frame": observation.local_frame.0,
                        "scale": transform.scale,
                        "offset_x": transform.offset_x,
                        "offset_y": transform.offset_y,
                    }))
                    .collect::<Vec<_>>(),
            },
            "prepared_edit_plan": {
                "plan_id": plan.id,
                "expected_revision": revision,
                "preview": plan.preview,
            },
        });
        Ok(success_structured(
            format!(
                "tracked mask effect {} on clip {} across {} samples as edit plan {}; inspect the preview, then commit it at timeline revision {revision}",
                args.effect_id,
                args.clip_id,
                observations.len(),
                plan.id,
            ),
            structured,
        ))
    }

    /// Measure one colour node's matte coverage at one exact project frame
    /// (CC5 §4.2).
    ///
    /// Read-only: it renders a scratch proof through the analysis backend and
    /// mutates nothing at all.
    #[allow(clippy::too_many_lines)]
    pub(super) fn inspect_grade_matte(
        &self,
        args: &InspectGradeMatteArgs,
    ) -> Result<CallToolResult, McpError> {
        let (revision, document) = self.snapshot()?;
        if let Some(expected) = args.expected_revision
            && expected != revision
        {
            return Ok(revision_conflict_text(expected, revision));
        }
        let Some(clip) = document.clip(args.clip_id) else {
            return Ok(matte_error_result(
                "matte_clip_not_found",
                &format!("clip {} does not exist", args.clip_id),
                &serde_json::json!({
                    "field": "clip_id",
                    "observed": args.clip_id.0,
                    "allowed": "an existing clip id",
                    "recovery_action": "Call get_timeline_state or get_color_context for the current clip ids.",
                }),
            ));
        };
        let Some(effect) = clip
            .effects
            .iter()
            .find(|effect| effect.id == args.effect_id)
        else {
            return Ok(matte_error_result(
                "matte_effect_not_found",
                &format!(
                    "effect {} does not exist on clip {}",
                    args.effect_id, args.clip_id
                ),
                &serde_json::json!({
                    "field": "effect_id",
                    "observed": args.effect_id.0,
                    "allowed": "an effect id on the requested clip",
                    "recovery_action": "Call get_color_context for the clip's colour_nodes.",
                    "clip_id": args.clip_id.0,
                }),
            ));
        };
        let Some(kind) = kinewright_core::classify_color_node(effect) else {
            return Ok(matte_error_result(
                "matte_effect_not_a_color_node",
                &format!("effect {} is {}", args.effect_id, effect.name),
                &serde_json::json!({
                    "field": "effect_id",
                    "observed": {"effect_id": args.effect_id.0, "name": effect.name},
                    "allowed": crate::color_status::MATTE_CAPABLE_NODE_NAMES,
                    // CC5 §1: the layer `mask` effect is a compositing alpha
                    // operation, not a colour node, and is never a secondary.
                    "recovery_action": "A matte belongs to a managed correction node. The layer `mask` effect is a compositing alpha operation, not a matte; inspect it with get_clip_info.",
                }),
            ));
        };
        if !kind.supports_matte() {
            return Ok(matte_error_result(
                "matte_unsupported_node_kind",
                &format!("{} cannot carry a matte", kind.effect_name()),
                &serde_json::json!({
                    "field": "effect_id",
                    "observed": kind.effect_name(),
                    "allowed": crate::color_status::MATTE_CAPABLE_NODE_NAMES,
                    "recovery_action": "A technical input transform normalizes the whole source, so a partially applied one is not a meaningful state (CC5 §2.1).",
                }),
            ));
        }

        // CC5 §2.6: every inactivity question is answered on the *evaluated*
        // stored integers, never on floats and never on the authored values.
        let clip_local = args
            .timecode
            .0
            .checked_sub(clip.timeline_start.0)
            .map_or(TimeCode::ZERO, TimeCode);
        let evaluated = effect.evaluated_at(clip_local);
        let matte = kinewright_core::MatteParams::from_effect(&evaluated);
        let inactive_reason = kinewright_core::color_node_inactive_reason(&evaluated);
        let resolved = matte_parameter_object(&matte);

        let image = match self.analysis.matte_proof_for_document(
            Arc::clone(&document),
            args.timecode,
            args.clip_id,
            args.effect_id,
        ) {
            Ok(proof) => proof,
            Err(error) => {
                // The engine may not implement matte proofs yet, and a node
                // that is inactive or matte-free fails typed rather than
                // returning a blank frame (CC5 §4.1). Both surface here as one
                // stable code with the backend's own message attached, so a
                // caller never mistakes "could not measure" for "empty".
                return Ok(matte_error_result(
                    crate::color_status::MATTE_PROOF_UNAVAILABLE,
                    &format!(
                        "could not render a matte proof for effect {} on clip {} at project frame {}: {error}",
                        args.effect_id, args.clip_id, args.timecode
                    ),
                    &serde_json::json!({
                        "field": "effect_id",
                        "observed": {
                            "effect_id": args.effect_id.0,
                            "clip_id": args.clip_id.0,
                            "project_frame": args.timecode.0,
                            "message": error.to_string(),
                            "node_kind": kind.effect_name(),
                            "active": inactive_reason.is_none(),
                            "inactive_reason": inactive_reason.map(kinewright_core::ColorNodeInactiveReason::as_str),
                            "has_matte": matte.has_matte(),
                        },
                        "allowed": "an active matte-carrying colour node rendered by a backend that implements matte proofs",
                        "recovery_action": "Enable the node's matte with plan_secondary_correction, clear its bypass, or retry once this build's renderer supports matte proofs; no coverage is invented here.",
                        "resolved_matte": resolved,
                    }),
                ));
            }
        };

        let statistics = match kinewright_core::matte_coverage_statistics(&image.coverage) {
            Ok(statistics) => statistics,
            Err(error) => {
                return Ok(matte_error_result(
                    error.code(),
                    &error.to_string(),
                    &serde_json::json!({
                        "field": "coverage",
                        "observed": error.to_string(),
                        "allowed": "a coverage raster with R = G = B and an opaque alpha (CC5 §4.1)",
                        "recovery_action": "The renderer returned a raster that is not a coverage proof; report this build's provenance.",
                    }),
                ));
            }
        };

        let include_image = args.include_image.unwrap_or(true);
        let png = if include_image {
            Some(encode_png(&image.coverage)?)
        } else {
            None
        };
        let structured = serde_json::json!({
            "timeline_revision": revision.0,
            "clip_id": args.clip_id.0,
            "effect_id": args.effect_id.0,
            "project_frame": args.timecode.0,
            "clip_local_frame": clip_local.0,
            "kind": kind.effect_name(),
            "role": kind.role(),
            "color_stage": kind.stage().as_str(),
            // CC5 §1: the two coverage concepts are named apart on every
            // surface, so a reader cannot mistake one for the other.
            "surface": "Matte (this correction)",
            "distinct_from": "Mask (layer alpha), which is a compositing operation and is never a CC1 secondary",
            "active": inactive_reason.is_none(),
            "inactive_reason": inactive_reason.map_or(serde_json::Value::Null, |reason| serde_json::json!(reason.as_str())),
            "matte": crate::color_status::matte_manifest_value(&evaluated),
            "resolved_matte_parameters": resolved,
            "statistics": statistics,
            // CC5 §4.3's threshold, restated at the level a caller reads it.
            "matte_threshold": kinewright_core::MATTE_SCOPE_THRESHOLD,
            "covered_pixel_count": statistics.covered_pixel_count,
            "raster": {
                "width": image.coverage.width,
                "height": image.coverage.height,
            },
            "raster_aspect_millionths": image.metadata.raster_aspect_millionths,
            "coverage_encoding": image.metadata.coverage_encoding,
            "coverage_scale": image.metadata.coverage_scale,
            "coverage_histogram_buckets": kinewright_core::MATTE_COVERAGE_HISTOGRAM_BUCKETS,
            "provenance": {
                "render": image.metadata.render,
                "clip_id": image.metadata.clip.0,
                "effect_id": image.metadata.effect.0,
                "node_kind": image.metadata.node_kind,
                "matte_enabled": image.metadata.matte_enabled,
                "window_count": image.metadata.window_count,
                "qualifier_enabled": image.metadata.qualifier_enabled,
            },
            "image_included": include_image,
            "evidence_only": true,
            "applied": false,
        });
        let mut content = vec![ContentBlock::text(format!(
            "matte coverage clip={} effect={} kind={} project_frame={} covered={}/{} pixels ({} bp)",
            args.clip_id,
            args.effect_id,
            kind.effect_name(),
            args.timecode,
            statistics.covered_pixel_count,
            statistics.total_pixel_count,
            statistics.covered_basis_points,
        ))];
        if let Some(png) = png {
            content.push(ContentBlock::image(BASE64.encode(png), "image/png"));
        }
        let mut result = CallToolResult::success(content);
        result.structured_content = Some(structured);
        Ok(result)
    }
    /// Track one matte window through a clip and return an unapplied keyframe
    /// plan (CC5 §5.2).
    ///
    /// Commits nothing: the two `SetEffectKeyframes` operations are returned
    /// as a prepared edit plan, exactly like `track_mask_region`.
    #[allow(clippy::too_many_lines)]
    pub(super) fn track_matte_window(
        &self,
        args: &TrackMatteWindowArgs,
    ) -> Result<CallToolResult, McpError> {
        let (revision, document) = self.snapshot()?;
        if let Some(expected) = args.expected_revision
            && expected != revision
        {
            return Ok(revision_conflict_text(expected, revision));
        }
        let Some(clip) = document.clip(args.clip_id) else {
            return Ok(error_text(format!("clip {} does not exist", args.clip_id)));
        };
        if !matches!(clip.content, ClipContent::Media) {
            return Ok(error_text("matte window tracking requires a media clip"));
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
        let Some(kind) = kinewright_core::classify_color_node(effect) else {
            return Ok(error_text(format!(
                "effect {} is {}; matte window tracking requires a matte-capable colour node",
                args.effect_id, effect.name
            )));
        };
        if !kind.supports_matte() {
            return Ok(matte_error_result(
                "matte_unsupported_node_kind",
                &format!("{} cannot carry a matte", kind.effect_name()),
                &serde_json::json!({
                    "field": "effect_id",
                    "observed": kind.effect_name(),
                    "allowed": crate::color_status::MATTE_CAPABLE_NODE_NAMES,
                    "recovery_action": "Track a window on a primary_correction, color_wheels, color_curves, or creative_look node (CC5 §2.1).",
                }),
            ));
        }
        let window_index = usize::from(args.window_index);
        if window_index >= kinewright_core::MATTE_WINDOW_LIMIT {
            return Ok(matte_error_result(
                "matte_window_index_out_of_range",
                &format!("window_index {} is outside 0..=3", args.window_index),
                &serde_json::json!({
                    "field": "window_index",
                    "observed": args.window_index,
                    "allowed": {"min": 0, "max": kinewright_core::MATTE_WINDOW_LIMIT - 1},
                    "recovery_action": "A matte carries at most four windows (CC5 §2.2).",
                }),
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
        let minimum_confidence = args
            .minimum_confidence_basis_points
            .unwrap_or(DEFAULT_MATTE_TRACK_MINIMUM_CONFIDENCE_BASIS_POINTS);
        if !(0..=10_000).contains(&minimum_confidence) {
            return Ok(error_text(
                "minimum_confidence_basis_points must be in 0..=10000",
            ));
        }

        let Some(first_local) = sample_frames.first().copied() else {
            return Ok(error_text("tracking requires at least one sample"));
        };
        // CC5 §5.2: the window is stored in *layer* uv while the tracker
        // measures the *composite*, so the layer transform must be resolvable
        // and static across the tracked range. A keyframed scale or offset
        // would make one composite pixel mean a different layer position at
        // every sample, which no single conversion can express.
        let transform = match resolve_static_layer_transform(effect_chain(clip), &sample_frames) {
            Ok(transform) => transform,
            Err(unsupported) => {
                return Ok(matte_error_result(
                    "matte_track_layer_transform_unsupported",
                    &format!(
                        "clip {} keyframes its layer {} over the tracked range",
                        args.clip_id, unsupported.field
                    ),
                    &serde_json::json!({
                        "field": unsupported.field,
                        "observed": unsupported.observed,
                        "allowed": "a layer scale and offset that resolve to one value across the whole tracked range",
                        "recovery_action": "Clear the transform automation over the tracked range, or track a range across which the layer transform is static; the matte window is matched with one template of one fixed size, and CC5 §5.2 requires a static layer transform over the tracked range so that template — and the window it produces — is reproducible.",
                        "clip_id": args.clip_id.0,
                        "range": {"start": start.0, "end": end.0},
                    }),
                ));
            }
        };

        let evaluated = effect.evaluated_at(first_local);
        let matte = kinewright_core::MatteParams::from_effect(&evaluated);
        if window_index >= matte.window_count {
            return Ok(matte_error_result(
                "matte_window_not_active",
                &format!(
                    "effect {} resolves matte_window_count {} at clip-local frame {first_local}, so window {} renders nothing",
                    args.effect_id, matte.window_count, args.window_index
                ),
                &serde_json::json!({
                    "field": "window_index",
                    "observed": args.window_index,
                    "allowed": {"max_active_index": matte.window_count.saturating_sub(1), "window_count": matte.window_count},
                    // CC5 §2.2: a window at index >= window_count is preserved
                    // but never rendered, so tracking it would animate geometry
                    // that affects no pixel.
                    "recovery_action": "Raise matte_window_count with plan_secondary_correction so the window renders, then track it.",
                }),
            ));
        }
        let Some(window) = matte.window(window_index).copied() else {
            return Ok(error_text("matte window index is outside 0..=3"));
        };

        // CC5 §5.2: the tracking box is the window's axis-aligned bounding box
        // mapped into the composited thumbnail, so it is rescaled by the layer
        // scale. `box_percent` is a full width/height, hence the factor two.
        let box_percent = [
            matte_track_box_percent(window.half_width_bp, transform.scale),
            matte_track_box_percent(window.half_height_bp, transform.scale),
        ];
        if box_percent.iter().any(|value| !(1..=75).contains(value)) {
            return Ok(matte_error_result(
                "matte_track_window_size_unsupported",
                &format!(
                    "window {} maps to a {}x{} percent template on the composite",
                    args.window_index, box_percent[0], box_percent[1]
                ),
                &serde_json::json!({
                    "field": "window_index",
                    "observed": {
                        "box_percent": box_percent,
                        "half_width_basis_points": window.half_width_bp,
                        "half_height_basis_points": window.half_height_bp,
                        "layer_scale": transform.scale,
                    },
                    "allowed": {"min_percent": 1, "max_percent": 75},
                    "recovery_action": "Shrink the window's half extents to bound the subject before tracking; a template covering most of the frame has no distinguishing content to match.",
                }),
            ));
        }
        let center_percent = match composite_seed_percent(
            transform,
            [
                basis_points_to_unit(window.center_x_bp),
                basis_points_to_unit(window.center_y_bp),
            ],
        ) {
            Ok(percent) => percent,
            Err(seed) => {
                // The repairable input is the window's own stored centre, not
                // the index that selected it, so the refusal names the offending
                // parameter and keeps the index as context.
                let index = args.window_index;
                return Ok(tracking_seed_outside_composite_result(
                    [
                        &format!("matte_window{index}_center_x_basis_points"),
                        &format!("matte_window{index}_center_y_basis_points"),
                    ],
                    args.clip_id,
                    first_local,
                    transform,
                    &seed,
                    &[("window_index", serde_json::json!(index))],
                ));
            }
        };

        let tracked = match self.track_clip_region(&RegionTrackingRequest {
            document: &document,
            clip_id: args.clip_id,
            clip_timeline_start: clip.timeline_start,
            sample_frames: &sample_frames,
            center_percent,
            box_percent,
            search_radius_percent: search_radius,
            max_width,
            // CC5 §5.2: excluding *this exact node* by id removes the feedback
            // a matte-scoped correction would otherwise create — as the window
            // moves the graded picture changes inside it and a SAD template
            // would chase its own output — while leaving every other grade and
            // every other effect, including a second node of the same kind,
            // intact.
            excluded_effect: args.effect_id,
        }) {
            Ok(tracked) => tracked,
            Err(error) => return Ok(error_text(error)),
        };

        let mut observations = Vec::new();
        let mut low_confidence_samples = Vec::new();
        for observation in &tracked.observations {
            let composite = [
                matte_track_centre_basis_points(observation.center[0], tracked.width),
                matte_track_centre_basis_points(observation.center[1], tracked.height),
            ];
            let layer = transform.composite_to_layer_basis_points(composite);
            let record = serde_json::json!({
                "local_frame": observation.local_frame.0,
                "project_frame": observation.project_frame.0,
                "center_x_basis_points": layer[0],
                "center_y_basis_points": layer[1],
                "composite_center_x_basis_points": composite[0],
                "composite_center_y_basis_points": composite[1],
                "center_pixel": observation.center,
                "confidence_basis_points": observation.confidence_basis_points,
            });
            if i64::from(observation.confidence_basis_points) < minimum_confidence {
                low_confidence_samples.push(record);
                continue;
            }
            observations.push((observation.local_frame, layer, record));
        }

        // CC5 §5.2: two surviving samples is the minimum a Linear curve can be
        // built from, and the roadmap's manual fallback is the recovery.
        if observations.len() < MATTE_TRACK_MINIMUM_SAMPLES {
            return Ok(matte_error_result(
                "tracking_confidence_too_low",
                &format!(
                    "only {} of {} samples reached {minimum_confidence} basis points of confidence",
                    observations.len(),
                    tracked.observations.len()
                ),
                &serde_json::json!({
                    "field": "minimum_confidence_basis_points",
                    "observed": {
                        "surviving_samples": observations.len(),
                        "total_samples": tracked.observations.len(),
                        "minimum_confidence_basis_points": minimum_confidence,
                        "low_confidence_samples": low_confidence_samples,
                    },
                    "allowed": {"minimum_surviving_samples": MATTE_TRACK_MINIMUM_SAMPLES},
                    "recovery_action": "Lower minimum_confidence_basis_points, shorten the tracked range, raise max_width, or set the window keyframes by hand; the tracker has no occlusion handling and will not invent a position it did not measure.",
                }),
            ));
        }

        // CC5 §5.2 / M40: raw tracker centres stutter, and tracker noise must
        // not become visible matte motion. The dead zone is deliberately zero
        // - a dead zone lags, which is right for a virtual camera and wrong for
        // a matte, which must stay on the subject.
        let smoothed = [0_usize, 1].map(|axis| {
            kinewright_core::stabilize_tracked_centres_basis_points(
                &observations
                    .iter()
                    .map(|(_, layer, _)| layer[axis])
                    .collect::<Vec<_>>(),
                kinewright_core::MATTE_WINDOW_CENTER_MIN_BASIS_POINTS,
                kinewright_core::MATTE_WINDOW_CENTER_MAX_BASIS_POINTS,
                MATTE_TRACK_DEAD_ZONE_BASIS_POINTS,
                MATTE_TRACK_MAX_STEP_BASIS_POINTS,
            )
        });

        let Some(names) = kinewright_core::matte_window_parameter_names(window_index) else {
            return Ok(error_text("matte window index is outside 0..=3"));
        };
        let parameter = |suffix: &str| {
            names
                .iter()
                .find(|name| name.ends_with(suffix))
                .copied()
                .unwrap_or_default()
                .to_owned()
        };
        let curve_for = |axis: usize| AutomationCurve {
            keyframes: observations
                .iter()
                .enumerate()
                .map(|(index, (local_frame, _, _))| Keyframe {
                    at: *local_frame,
                    value: smoothed[axis].get(index).copied().unwrap_or_default(),
                    // CC5 §5.2: sustained movement gets continuous velocity;
                    // M40 rejected eased per-segment curves.
                    interpolation: KeyframeInterpolation::Linear,
                })
                .collect(),
        };
        let x_name = parameter("_center_x_basis_points");
        let y_name = parameter("_center_y_basis_points");
        let x_curve = curve_for(0);
        let y_curve = curve_for(1);
        let operations = vec![
            Operation::SetEffectKeyframes {
                clip: args.clip_id,
                effect: args.effect_id,
                name: x_name.clone(),
                curve: x_curve.clone(),
            },
            Operation::SetEffectKeyframes {
                clip: args.clip_id,
                effect: args.effect_id,
                name: y_name.clone(),
                curve: y_curve.clone(),
            },
        ];
        let plan = match self.prepare_operations(revision, &document, operations) {
            Ok(plan) => plan,
            Err(error) => {
                return Ok(error_text(format!(
                    "tracked matte window keyframes do not fit the current clip: {error}"
                )));
            }
        };

        let structured = serde_json::json!({
            "timeline_revision": revision.0,
            "clip_id": args.clip_id.0,
            "effect_id": args.effect_id.0,
            "kind": kind.effect_name(),
            "window_index": args.window_index,
            "range": {"start": start.0, "end": end.0, "step_frames": step},
            "observations": observations
                .iter()
                .map(|(_, _, record)| record.clone())
                .collect::<Vec<_>>(),
            "low_confidence_samples": low_confidence_samples,
            "minimum_confidence_basis_points": minimum_confidence,
            "curves": {
                x_name.clone(): x_curve,
                y_name.clone(): y_curve,
            },
            "parameters": [x_name, y_name],
            // CC5 §5.2: the pinned M40 smoothing policy, published so a reader
            // can reproduce the smoothed curve from the raw observations.
            "window_stabilization": {
                "median_filter": true,
                "dead_zone_basis_points": MATTE_TRACK_DEAD_ZONE_BASIS_POINTS,
                "maximum_step_basis_points": MATTE_TRACK_MAX_STEP_BASIS_POINTS,
                "minimum_basis_points": kinewright_core::MATTE_WINDOW_CENTER_MIN_BASIS_POINTS,
                "maximum_basis_points": kinewright_core::MATTE_WINDOW_CENTER_MAX_BASIS_POINTS,
                "interpolation": "Linear",
                "known_systematic_lag": "the three-sample median filter replaces the final sample with median(o[n-3], o[n-2], o[n-1]), so the last smoothed value lags a moving subject by one inter-sample displacement (CC5 §5.2)",
            },
            "coordinate_space": {
                "measured_on": "composited thumbnail, whose uv is the output frame",
                "written_in": "layer uv, which is where the matte is evaluated",
                "thumbnail": {"width": tracked.width, "height": tracked.height},
                "layer_scale": transform.scale,
                "layer_offset": [transform.offset_x, transform.offset_y],
                "pixel_to_basis_points": "centre_bp = round((pixel + 0.5) * 10000 / extent)",
                "composite_to_layer": "u_layer = (u_composite - 0.5) / scale - (offset_x, offset_y) / (2 * scale) + 0.5",
                "box_percent": box_percent,
                "box_percent_rule": "the window bounding box rescaled by the layer scale: box_percent = [2 * hw * scale * 100, 2 * hh * scale * 100] (CC5 §5.2)",
            },
            // CC5 §5.2's provenance marker, mirroring M40's.
            "tracking_boundary": MATTE_TRACKING_BOUNDARY,
            "prepared_edit_plan": {
                "plan_id": plan.id,
                "expected_revision": revision,
                "preview": plan.preview,
            },
            "applied": false,
        });
        Ok(success_structured(
            format!(
                "tracked matte window {} on effect {} of clip {} across {} samples as edit plan {}; inspect the preview, then commit it at timeline revision {revision}",
                args.window_index,
                args.effect_id,
                args.clip_id,
                observations.len(),
                plan.id,
            ),
            structured,
        ))
    }
}

/// A dead zone deliberately lags. That is right for a virtual camera and wrong
/// for a matte, which must stay on the subject (CC5 §5.2).
pub(crate) const MATTE_TRACK_DEAD_ZONE_BASIS_POINTS: i64 = 0;

/// 8 % of the frame between samples; at the default 5-frame step, 1.6 % per
/// frame — well above ordinary subject motion, while still rejecting a tracker
/// jump to a false match across the frame (CC5 §5.2).
pub(crate) const MATTE_TRACK_MAX_STEP_BASIS_POINTS: i64 = 800;

/// Default confidence floor below which a tracked sample is dropped.
const DEFAULT_MATTE_TRACK_MINIMUM_CONFIDENCE_BASIS_POINTS: i64 = 5_000;

/// Fewer surviving samples than this cannot describe motion, so the tool fails
/// typed rather than emitting a one-point curve (CC5 §5.2).
const MATTE_TRACK_MINIMUM_SAMPLES: usize = 2;

/// CC5 §5.2's provenance marker, stated so the tool's reach is not inferred.
pub(super) const MATTE_TRACKING_BOUNDARY: &str = "tracks the explicitly supplied window rectangle by normalized SAD template match on composited thumbnails; no learned object, face, or skin detection, no scale or rotation estimation, and no occlusion handling. rotation_centidegrees, half_width_basis_points, and half_height_basis_points are never written.";

/// One layer's resolved geometric transform over a tracked range (CC5 §5.2).
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct LayerTransform {
    /// Product of every `scale_percent / 100` on the layer.
    pub(super) scale: f64,
    /// Sum of every `x_percent / 50`, in the compositor's own units.
    pub(super) offset_x: f64,
    /// Sum of every `y_percent / 50`, in the compositor's own units.
    pub(super) offset_y: f64,
}

impl LayerTransform {
    /// The identity layer: no scale, no offset.
    pub(super) const IDENTITY: Self = Self {
        scale: 1.0,
        offset_x: 0.0,
        offset_y: 0.0,
    };

    /// Map a layer uv to the composited frame's uv (CC5 §5.2).
    ///
    /// Derived from `compositor.wgsl`'s vertex stage, which places the layer
    /// quad at NDC `p = q·scale + (offset_x, −offset_y)` and hands the
    /// fragment stage `uv.y = (1 − ndc.y)/2`. The two sign flips on y — the
    /// shader's own negation and the flip built into the uv convention —
    /// cancel exactly, so **both** axes carry `+offset/2`:
    ///
    /// `u_composite = scale·(u_layer − 0.5) + (offset_x, offset_y)/2 + 0.5`
    ///
    /// `offset_x`/`offset_y` are the compositor's own accumulated units, i.e.
    /// `sum(percent) / 50`, exactly as `EffectUniform::OffsetX`/`OffsetY` are
    /// accumulated by the compositor.
    pub(super) fn layer_to_composite(self, layer: [f64; 2]) -> [f64; 2] {
        [
            (layer[0] - 0.5).mul_add(self.scale, self.offset_x / 2.0) + 0.5,
            (layer[1] - 0.5).mul_add(self.scale, self.offset_y / 2.0) + 0.5,
        ]
    }

    /// CC5 §5.2's normative composite → layer conversion, in normalized uv.
    ///
    /// The exact inverse of [`Self::layer_to_composite`], **unclamped**:
    ///
    /// `u_layer = (u_composite − 0.5)/scale − (offset_x, offset_y)/(2·scale) + 0.5`
    ///
    /// No clamp, deliberately: a layer scaled below 1 occupies only part of the
    /// composite, so composite coordinates outside the layer's own quad map to
    /// layer coordinates outside `0..=1`, and every caller decides for itself
    /// what to do with them. A degenerate `scale <= 0` collapses the quad and
    /// has no inverse, so the composite coordinate is returned unchanged rather
    /// than divided by zero.
    pub(super) fn composite_to_layer_unit(self, unit: [f64; 2]) -> [f64; 2] {
        let convert = |value: f64, offset: f64| {
            if self.scale <= 0.0 {
                return value;
            }
            (value - 0.5 - offset / 2.0) / self.scale + 0.5
        };
        [
            convert(unit[0], self.offset_x),
            convert(unit[1], self.offset_y),
        ]
    }

    /// [`Self::composite_to_layer_unit`] in basis points, clamped to CC5
    /// §2.2's matte window centre range.
    pub(super) fn composite_to_layer_basis_points(self, composite: [i64; 2]) -> [i64; 2] {
        #[allow(clippy::cast_precision_loss)]
        let unit = self.composite_to_layer_unit([
            composite[0] as f64 / 10_000.0,
            composite[1] as f64 / 10_000.0,
        ]);
        unit.map(|layer| {
            #[allow(clippy::cast_possible_truncation)]
            let basis_points = (layer * 10_000.0).round() as i64;
            basis_points.clamp(
                kinewright_core::MATTE_WINDOW_CENTER_MIN_BASIS_POINTS,
                kinewright_core::MATTE_WINDOW_CENTER_MAX_BASIS_POINTS,
            )
        })
    }
}

/// A layer-transform parameter that moves across the tracked range.
pub(super) struct LayerTransformUnsupported {
    pub(super) field: &'static str,
    pub(super) observed: serde_json::Value,
}

/// Resolve one layer's scale and offset at exactly one frame.
///
/// The accumulation is the compositor's own, restated: `params_for` multiplies
/// every `EffectUniform::Scale` by `value / 100` and adds every
/// `EffectUniform::OffsetX` / `OffsetY` as `value / 50`, over the whole effect
/// chain in order, with a missing parameter taking the descriptor's neutral
/// value. Resolving per frame is what lets CC5 §5.2's composite → layer
/// conversion follow a *keyframed* transform: the map is affine at each
/// instant even when it moves between instants.
pub(super) fn resolve_layer_transform_at(effects: &[Effect], frame: TimeCode) -> LayerTransform {
    let mut transform = LayerTransform::IDENTITY;
    for effect in effects {
        let Some(descriptor) = kinewright_core::effect_descriptor(&effect.name) else {
            continue;
        };
        for parameter in descriptor.parameters {
            let value = effect
                .integer_parameter_at(parameter.name, frame)
                .unwrap_or(parameter.neutral);
            #[allow(clippy::cast_precision_loss)]
            let value = value as f64;
            match parameter.uniform {
                kinewright_core::EffectUniform::Scale => transform.scale *= value / 100.0,
                kinewright_core::EffectUniform::OffsetX => transform.offset_x += value / 50.0,
                kinewright_core::EffectUniform::OffsetY => transform.offset_y += value / 50.0,
                _ => {}
            }
        }
    }
    transform
}

/// Resolve one layer's static scale and offset across every sampled frame.
///
/// CC5 §5.2 requires the composite → layer conversion to be one affine map, so
/// a `scale` or `offset` whose resolved value differs between samples is a
/// typed refusal rather than a silently-wrong conversion. A keyframe curve that
/// happens to resolve to one constant value is accepted: the rule is about the
/// values the renderer uses, not about the presence of automation.
pub(super) fn resolve_static_layer_transform(
    effects: &[Effect],
    sample_frames: &[TimeCode],
) -> Result<LayerTransform, LayerTransformUnsupported> {
    let mut resolved: Option<LayerTransform> = None;
    for frame in sample_frames {
        let transform = resolve_layer_transform_at(effects, *frame);
        match resolved {
            None => resolved = Some(transform),
            Some(first) => {
                for (field, first_value, value) in [
                    ("scale", first.scale, transform.scale),
                    ("offset_x", first.offset_x, transform.offset_x),
                    ("offset_y", first.offset_y, transform.offset_y),
                ] {
                    if (first_value - value).abs() > f64::EPSILON {
                        return Err(LayerTransformUnsupported {
                            field,
                            observed: serde_json::json!({
                                "parameter": field,
                                "at_first_sample": first_value,
                                "at_frame": frame.0,
                                "value_at_frame": value,
                            }),
                        });
                    }
                }
            }
        }
    }
    Ok(resolved.unwrap_or(LayerTransform::IDENTITY))
}

/// The clip's effect chain, borrowed for transform resolution.
pub(super) fn effect_chain(clip: &Clip) -> &[Effect] {
    &clip.effects
}

/// One resolved layer scale and the sampled frame it was resolved at.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct LayerScaleAt {
    pub(super) frame: TimeCode,
    pub(super) scale: f64,
}

/// The smallest and largest layer scale resolved over `sample_frames`.
///
/// CC5 §5.2: the tracking template is sized *once*, at the seed frame's scale,
/// while the composite → layer conversion is redone per frame. A keyframed
/// scale therefore makes the *same* template legal at one end of the range and
/// illegal at the other, so the `1..=75` template gate is applied at both
/// extremes rather than at the seed alone. Returns `None` for an empty range.
pub(super) fn layer_scale_extremes(
    effects: &[Effect],
    sample_frames: &[TimeCode],
) -> Option<(LayerScaleAt, LayerScaleAt)> {
    let mut minimum: Option<LayerScaleAt> = None;
    let mut maximum: Option<LayerScaleAt> = None;
    for frame in sample_frames {
        let resolved = LayerScaleAt {
            frame: *frame,
            scale: resolve_layer_transform_at(effects, *frame).scale,
        };
        if minimum.is_none_or(|current| resolved.scale < current.scale) {
            minimum = Some(resolved);
        }
        if maximum.is_none_or(|current| resolved.scale > current.scale) {
            maximum = Some(resolved);
        }
    }
    minimum.zip(maximum)
}

/// The first resolved scale at which a stored region is an illegal template.
///
/// The template gate is CC5 §5.2's `1..=75` percent of the composited frame.
/// [`tracked_box_percent`] is monotone in the scale, so a region that is legal
/// at both the smallest and the largest resolved scale is legal at every scale
/// between them. Returns the offending scale, the frame it was resolved at, and
/// the template that scale produces.
pub(super) fn offending_template_scale(
    stored_percent: [i64; 2],
    extremes: (LayerScaleAt, LayerScaleAt),
) -> Option<(LayerScaleAt, [i64; 2])> {
    let (minimum, maximum) = extremes;
    [minimum, maximum].into_iter().find_map(|resolved| {
        let template = [
            tracked_box_percent(stored_percent[0], resolved.scale),
            tracked_box_percent(stored_percent[1], resolved.scale),
        ];
        template
            .iter()
            .any(|value| !(1..=75).contains(value))
            .then_some((resolved, template))
    })
}

/// CC5 §5.2's exact statement of what the per-frame transform does and does not
/// cover, for the `coordinate_space` block of both region trackers.
///
/// The *conversion* is redone at every sampled frame; the *template* is sized
/// once, at the seed frame's scale, and is gated against the whole resolved
/// range. Stating the seed scale and both extremes keeps the claim falsifiable.
pub(super) fn keyframed_transform_note(
    seed_scale: f64,
    extremes: Option<(LayerScaleAt, LayerScaleAt)>,
) -> String {
    let range = extremes.map_or_else(
        || "no sampled frames".to_owned(),
        |(minimum, maximum)| {
            format!(
                "{} at clip-local frame {} to {} at clip-local frame {}",
                minimum.scale, minimum.frame, maximum.scale, maximum.frame
            )
        },
    );
    format!(
        "the composite-to-layer conversion is resolved at every sampled frame, so a keyframed scale or offset is converted sample by sample rather than refused; the tracking template itself is sized once, at the seed frame's scale {seed_scale}, and the 1..=75 percent template gate is applied across the resolved scale range {range}"
    )
}

/// A seed centre whose forward map lands outside the composited frame.
pub(super) struct TrackingSeedOutsideComposite {
    layer: [f64; 2],
    composite: [f64; 2],
}

/// Push one layer-space seed centre forward onto the composite (CC5 §5.2).
///
/// The tracker searches the composited thumbnail, so a seed that maps outside
/// `0..=1` names no pixel at all. Clamping it to the raster edge would silently
/// track whatever happens to sit in the corner, so the caller refuses instead.
pub(super) fn composite_seed_percent(
    transform: LayerTransform,
    layer: [f64; 2],
) -> Result<[u8; 2], TrackingSeedOutsideComposite> {
    let composite = transform.layer_to_composite(layer);
    if composite.iter().any(|unit| !(0.0..=1.0).contains(unit)) {
        return Err(TrackingSeedOutsideComposite { layer, composite });
    }
    Ok([unit_to_percent(composite[0]), unit_to_percent(composite[1])])
}

/// CC5 §5.2's typed refusal for a seed that leaves the composited frame.
///
/// `axis_fields` names the two *caller-editable parameters* the seed came from,
/// horizontal first. The published `field` is the one whose axis actually left
/// `0..=1`, or both of them when both did, so an agent can repair the exact
/// input rather than being handed a generic selector. `extra_observed` carries
/// any caller-specific context — `track_matte_window`'s `window_index`, say —
/// into `observed`, where it belongs now that it no longer names the field.
pub(super) fn tracking_seed_outside_composite_result(
    axis_fields: [&str; 2],
    clip: ClipId,
    frame: TimeCode,
    transform: LayerTransform,
    seed: &TrackingSeedOutsideComposite,
    extra_observed: &[(&str, serde_json::Value)],
) -> CallToolResult {
    let outside = [
        !(0.0..=1.0).contains(&seed.composite[0]),
        !(0.0..=1.0).contains(&seed.composite[1]),
    ];
    let field = match outside {
        [true, true] => serde_json::json!([axis_fields[0], axis_fields[1]]),
        [false, true] => serde_json::json!(axis_fields[1]),
        // A refusal is only raised when at least one axis is outside, so the
        // remaining arms both name the horizontal parameter.
        _ => serde_json::json!(axis_fields[0]),
    };
    let mut observed = serde_json::json!({
        "layer_center_unit": seed.layer,
        "composite_center_unit": seed.composite,
        "scale": transform.scale,
        "offset_x": transform.offset_x,
        "offset_y": transform.offset_y,
        "clip_local_frame": frame.0,
    });
    if let Some(map) = observed.as_object_mut() {
        for (name, value) in extra_observed {
            map.insert((*name).to_owned(), value.clone());
        }
    }
    matte_error_result(
        "tracking_seed_outside_composite",
        &format!(
            "clip {clip}'s layer transform at clip-local frame {frame} places the tracking seed at composite ({:.4}, {:.4}), outside the composited frame",
            seed.composite[0], seed.composite[1],
        ),
        &serde_json::json!({
            "field": field,
            "observed": observed,
            "allowed": "a seed whose forward-mapped composite centre lies in 0..=1 on both axes",
            "recovery_action": "Move the layer back inside the frame over the tracked range, or seed the tracker on a point that is actually visible; the tracker matches composited pixels and a seed off the raster names none (CC5 §5.2).",
            "clip_id": clip.0,
        }),
    )
}

/// CC5 §5.2's tracker-pixel to matte-basis-point conversion.
///
/// The tracker's own `pixel_to_basis_points` divides by `extent − 1`, because
/// it names a *sample position* on a lattice. A matte centre names a *fraction
/// of the extent*, so it divides by `extent` and adds the half-pixel that puts
/// the sample at the pixel centre. The two are deliberately different functions
/// and must not be interchanged; §9.2.11 records the ≤ 17 bp divergence.
pub(super) fn matte_track_centre_basis_points(pixel: u32, extent: u32) -> i64 {
    if extent == 0 {
        return 0;
    }
    // round((pixel + 0.5) * 10000 / extent), in exact integer arithmetic:
    // (2*pixel + 1) * 10000 / (2*extent), rounded half up by adding `extent`.
    let numerator = (u64::from(pixel).saturating_mul(2).saturating_add(1)).saturating_mul(10_000);
    let denominator = u64::from(extent).saturating_mul(2);
    i64::try_from(numerator.saturating_add(u64::from(extent)) / denominator).unwrap_or(10_000)
}

/// The tracking template width or height, as a whole frame percentage.
///
/// CC5 §5.2: the box is the window's bounding box *on the composite*, so a
/// half extent stored in layer basis points is doubled and rescaled by the
/// layer scale.
pub(super) fn matte_track_box_percent(half_extent_basis_points: i64, scale: f64) -> i64 {
    #[allow(clippy::cast_precision_loss)]
    let half = half_extent_basis_points as f64 / 10_000.0;
    #[allow(clippy::cast_possible_truncation)]
    let percent = (2.0 * half * scale * 100.0).round() as i64;
    percent
}

/// CC5 §5.2's tracker pixel as a *fraction of the composited extent*.
///
/// The float twin of [`matte_track_centre_basis_points`]:
/// `u_composite = (pixel + 0.5) / extent`.
///
/// Deliberately **not** [`pixel_to_percent`]'s `extent − 1` lattice
/// denominator. `mask_center_x` and `focus_x_basis_points` are read by the
/// compositor as fractions of the extent (`value / 100`, `value / 10000`), not
/// as sample positions on a lattice, so the two conversions are different
/// functions and must not be interchanged.
pub(super) fn tracker_pixel_to_composite_unit(pixel: u32, extent: u32) -> f64 {
    if extent == 0 {
        return 0.5;
    }
    (f64::from(pixel) + 0.5) / f64::from(extent)
}

/// One tracked composite pixel centre as a layer-space unit pair (CC5 §5.2).
pub(super) fn tracked_centre_layer_unit(
    center: [u32; 2],
    width: u32,
    height: u32,
    transform: LayerTransform,
) -> [f64; 2] {
    transform.composite_to_layer_unit([
        tracker_pixel_to_composite_unit(center[0], width),
        tracker_pixel_to_composite_unit(center[1], height),
    ])
}

/// A unit-square coordinate as a whole-percent control value, clamped to
/// `0..=100`.
///
/// Used on layer units to produce the values the plan writes, and on composite
/// units to publish the tracker's raw reading as provenance in the same
/// convention, so the two are directly comparable.
pub(super) fn layer_unit_to_percent(unit: f64) -> i64 {
    #[allow(clippy::cast_possible_truncation)]
    let percent = (unit * 100.0).round().clamp(0.0, 100.0) as i64;
    percent
}

/// A layer-space unit as a basis-point control value, clamped to `0..=10000`.
pub(super) fn layer_unit_to_basis_points(unit: f64) -> i64 {
    #[allow(clippy::cast_possible_truncation)]
    let basis_points = (unit * 10_000.0).round().clamp(0.0, 10_000.0) as i64;
    basis_points
}

/// A tracked template extent, rescaled from layer percent onto the composite.
///
/// The mask region and the reframe subject both state a *full* width or height
/// percent in layer space, while the tracker matches on the composite, where
/// the layer scale has already been applied. Mirrors
/// [`matte_track_box_percent`], whose input is a half extent in basis points.
pub(super) fn tracked_box_percent(full_percent: i64, scale: f64) -> i64 {
    #[allow(clippy::cast_precision_loss)]
    let percent = full_percent as f64 * scale;
    #[allow(clippy::cast_possible_truncation)]
    let rounded = percent.round() as i64;
    rounded
}

/// A `0..=10000` basis-point control as a `0.0..=1.0` fraction.
fn basis_points_to_unit(basis_points: i64) -> f64 {
    #[allow(clippy::cast_precision_loss)]
    let value = basis_points as f64;
    value / 10_000.0
}

/// A normalized coordinate as the tracker's whole-percent seed, clamped.
pub(super) fn unit_to_percent(unit: f64) -> u8 {
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let percent = (unit * 100.0).round().clamp(0.0, 100.0) as u8;
    percent
}

/// The CC5 §4.2 matte parameters as one compact integer object.
fn matte_parameter_object(matte: &kinewright_core::MatteParams) -> serde_json::Value {
    serde_json::json!({
        "matte_enabled": matte.enabled,
        "matte_window_count": matte.window_count,
        "matte_combine_token": matte.combine_token,
        "matte_invert": matte.invert,
        "matte_mix_basis_points": matte.mix_bp,
        "matte_qualifier_enabled": matte.qualifier.enabled,
        "matte_hue_center_centidegrees": matte.qualifier.hue_center_cd,
        "matte_hue_width_centidegrees": matte.qualifier.hue_width_cd,
        "matte_hue_softness_centidegrees": matte.qualifier.hue_softness_cd,
        "matte_saturation_low_basis_points": matte.qualifier.sat_low_bp,
        "matte_saturation_high_basis_points": matte.qualifier.sat_high_bp,
        "matte_saturation_softness_basis_points": matte.qualifier.sat_softness_bp,
        "matte_luma_low_basis_points": matte.qualifier.luma_low_bp,
        "matte_luma_high_basis_points": matte.qualifier.luma_high_bp,
        "matte_luma_softness_basis_points": matte.qualifier.luma_softness_bp,
        // CC5 §2.2: stored windows past the count render nothing, so only the
        // active ones are published.
        "windows": matte
            .active_windows()
            .enumerate()
            .map(|(index, window)| serde_json::json!({
                "index": index,
                "shape_token": window.shape_token,
                "center_x_basis_points": window.center_x_bp,
                "center_y_basis_points": window.center_y_bp,
                "half_width_basis_points": window.half_width_bp,
                "half_height_basis_points": window.half_height_bp,
                "rotation_centidegrees": window.rotation_cd,
                "feather_basis_points": window.feather_bp,
                "invert": window.invert,
            }))
            .collect::<Vec<_>>(),
    })
}

/// One typed CC5 refusal in the CC1/CC2 `field`/`observed`/`allowed` shape.
fn matte_error_result(code: &str, message: &str, details: &serde_json::Value) -> CallToolResult {
    error_structured(
        message.to_owned(),
        serde_json::json!({
            "code": code,
            "message": message,
            "details": details,
            "evidence_only": true,
            "applied": false,
        }),
    )
}

/// The *lattice* pixel-to-percent conversion, `pixel / (extent − 1)`.
///
/// No production path uses it any more: `track_mask_region` published its
/// composite provenance through it, which contradicted the
/// `u = (pixel + 0.5) / extent` map the same response declares, so it now goes
/// through [`tracker_pixel_to_composite_unit`] like every other CC5 §5.2 path.
/// Kept as the reference the §9.2.11 divergence test measures the two
/// denominators against, alongside [`pixel_to_basis_points`].
#[cfg(test)]
pub(super) fn pixel_to_percent(pixel: u32, extent: u32) -> u8 {
    let denominator = extent.saturating_sub(1).max(1);
    let rounded = pixel.saturating_mul(100).saturating_add(denominator / 2) / denominator;
    u8::try_from(rounded.min(100)).unwrap_or(100)
}

/// The tracker's *lattice* pixel-to-basis-point conversion, `pixel / (extent −
/// 1)`.
///
/// No production path uses it any more: CC5 §5.2 requires every written control
/// to be a fraction of the extent, so `track_reframe_subject` now goes through
/// [`tracker_pixel_to_composite_unit`] like `track_matte_window` does. It is
/// kept as the reference the §9.2.11 divergence test measures the two
/// denominators against.
#[cfg(test)]
pub(super) fn pixel_to_basis_points(pixel: u32, extent: u32) -> u16 {
    let denominator = u64::from(extent.saturating_sub(1).max(1));
    let rounded = u64::from(pixel)
        .saturating_mul(10_000)
        .saturating_add(denominator / 2)
        / denominator;
    u16::try_from(rounded.min(10_000)).unwrap_or(10_000)
}
