//! CC1/CC4/CC5 `render_color_proof` and its proof manifest helpers.

use super::storyboards::compose_contact_sheet;
use super::*;

impl KinewrightMcp {
    #[allow(clippy::too_many_lines)]
    pub(super) fn render_color_proof(
        &self,
        args: &RenderColorProofArgs,
    ) -> Result<CallToolResult, McpError> {
        let (actual_revision, document) = self.snapshot()?;
        let looks = self.look_context(&document);
        // CC4 §8: `effect_id` proofs the *stored* node, so a proposed-primary
        // parameter set alongside it would describe a different edit.
        if let Some(effect) = args.effect_id
            && !args.parameters.is_empty()
        {
            return Ok(color_proof_error_result(
                ColorProofError::LookProofParametersConflict { effect },
            ));
        }
        if args.look_comparison.is_some() && args.effect_id.is_none() {
            return Ok(color_proof_error_result(
                ColorProofError::LookComparisonRequiresEffectId,
            ));
        }
        // CC5 §7: `matte_comparison` is valid only alongside `effect_id`, and
        // both comparisons select what the AFTER cell renders, so exactly one
        // may be sent.
        if args.matte_comparison.is_some() && args.effect_id.is_none() {
            return Ok(color_proof_error_result(
                ColorProofError::MatteComparisonRequiresEffectId,
            ));
        }
        if args.matte_comparison.is_some() && args.look_comparison.is_some() {
            return Ok(color_proof_error_result(
                ColorProofError::MatteComparisonConflictsWithLookComparison,
            ));
        }
        // Resolve the stored node before any render work: an unrenderable
        // request must cost nothing.
        let stored_node = match args.effect_id {
            None => None,
            Some(effect_id) => {
                let Some(stored) = document
                    .clip(args.clip_id)
                    .and_then(|clip| clip.effects.iter().find(|effect| effect.id == effect_id))
                else {
                    return Ok(color_proof_error_result(
                        ColorProofError::ProofEffectNotFound {
                            clip: args.clip_id,
                            effect: effect_id,
                        },
                    ));
                };
                let Some(kind) = kinewright_core::classify_color_node(stored) else {
                    return Ok(color_proof_error_result(
                        ColorProofError::ProofEffectNotAColorNode {
                            effect: effect_id,
                            name: stored.name.clone(),
                        },
                    ));
                };
                // LUT nodes render through the `LutLibrary` the application
                // publishes on the media engine (`FfmpegMediaEngine::
                // set_lut_library`), which the proof renderer reads. An active
                // LUT node whose asset is not published fails the render with a
                // typed `missing_lut_asset:` error rather than a look-free frame.
                // CC3 §5: a CC1 primary carries no bypass control, so the
                // bypass variant is not a state this node can be put into.
                if matches!(args.look_comparison, Some(LookComparison::Bypass))
                    && kind == ColorNodeKind::Primary
                {
                    return Ok(color_proof_error_result(
                        ColorProofError::LookBypassUnsupported {
                            effect: effect_id,
                            kind: kind.effect_name(),
                        },
                    ));
                }
                // CC5 §7: a matte comparison needs a node that both may carry a
                // matte and actually does. Both are checked before any render
                // work, so an unrenderable request costs nothing.
                if args.matte_comparison.is_some() {
                    if !kind.supports_matte() {
                        return Ok(color_proof_error_result(
                            ColorProofError::MatteComparisonUnsupportedKind {
                                effect: effect_id,
                                kind: kind.effect_name(),
                            },
                        ));
                    }
                    let clip_local = args
                        .timecode
                        .0
                        .checked_sub(
                            document
                                .clip(args.clip_id)
                                .map_or(0, |clip| clip.timeline_start.0),
                        )
                        .map_or(TimeCode::ZERO, TimeCode);
                    if !kinewright_core::MatteParams::from_effect(&stored.evaluated_at(clip_local))
                        .has_matte()
                    {
                        return Ok(color_proof_error_result(
                            ColorProofError::MatteComparisonNoMatte { effect: effect_id },
                        ));
                    }
                }
                Some((effect_id, kind))
            }
        };
        let plan_args = PrimaryCorrectionPlanArgs::from(args);
        let plan = match plan_primary_correction(&document, actual_revision, &plan_args) {
            Ok(plan) => plan,
            Err(error) => {
                return Ok(color_proof_error_result(ColorProofError::from(error)));
            }
        };
        let look_comparison = args.look_comparison.unwrap_or(LookComparison::After);
        if !document.color_context.is_managed_sdr_compatible() {
            return Ok(color_proof_error_result(
                ColorProofError::PipelineIncompatible {
                    reason: format!(
                        "pipeline_state={:?}, working={:?}, monitoring={:?}",
                        document.color_context.pipeline_state,
                        document.color_context.working,
                        document.color_context.monitoring,
                    ),
                },
            ));
        }
        if args.timecode < TimeCode::ZERO || args.timecode >= document.duration {
            return Ok(color_proof_error_result(
                ColorProofError::ProjectFrameOutOfRange {
                    frame: args.timecode,
                    duration: document.duration,
                },
            ));
        }
        let Some(clip) = document.clip(args.clip_id) else {
            return Ok(color_proof_error_result(ColorProofError::Primary(
                PrimaryPlanError::MissingClip(args.clip_id),
            )));
        };
        let clip_duration = match document.clip_duration(clip) {
            Ok(duration) => duration,
            Err(error) => {
                return Ok(color_proof_error_result(
                    ColorProofError::ClipTimingInvalid {
                        clip: args.clip_id,
                        reason: error.to_string(),
                    },
                ));
            }
        };
        let Some(clip_end) = clip.timeline_start.checked_add(clip_duration) else {
            return Ok(color_proof_error_result(
                ColorProofError::ClipTimingInvalid {
                    clip: args.clip_id,
                    reason: "clip end overflowed".to_owned(),
                },
            ));
        };
        if args.timecode < clip.timeline_start || args.timecode >= clip_end {
            return Ok(color_proof_error_result(
                ColorProofError::ClipFrameOutOfRange {
                    clip: args.clip_id,
                    frame: args.timecode,
                    start: clip.timeline_start,
                    end: clip_end,
                },
            ));
        }
        let Some(asset) = document.asset(clip.asset) else {
            return Ok(color_proof_error_result(ColorProofError::Primary(
                PrimaryPlanError::MissingAsset {
                    clip: args.clip_id,
                    asset: clip.asset,
                },
            )));
        };
        // A proof renders one exact project frame.  Availability therefore
        // follows the compositor's active visual layers at that frame, not
        // every clip in the document (and never audio-only tracks).  This is
        // important for offline bins and for a later shot that is not part of
        // the requested BEFORE/AFTER image.
        let active_visual_layers =
            match kinewright_media::visual_layers_at(&document, args.timecode) {
                Ok(layers) => layers,
                Err(error) => {
                    return Ok(color_proof_error_result(ColorProofError::from_media_error(
                        "visual_layer_resolution",
                        error,
                    )));
                }
            };
        let mut active_rendered_layers = Vec::new();
        let mut active_rendered_sources = Vec::new();
        let mut unsupported_layer_warnings = Vec::new();
        let mut blocking_layer_source: Option<(ClipId, AssetId, ColorSourceError)> = None;
        let mut selected_clip_is_rendered = false;
        for layer in active_visual_layers {
            let (track_id, clip_id, asset_id) = match &layer {
                kinewright_media::TimelineVisualLayer::Video(layer) => (
                    layer.source.track,
                    layer.source.clip,
                    Some(layer.source.asset),
                ),
                kinewright_media::TimelineVisualLayer::Title(layer) => {
                    (layer.track, layer.clip, None)
                }
            };
            let Some(timeline_clip) = Self::document_clip_on_track(&document, track_id, clip_id)
            else {
                return Ok(color_proof_error_result(ColorProofError::RenderFailed {
                    stage: "visual_layer_resolution",
                    message: format!(
                        "production visual resolver returned track {track_id} clip {clip_id}, but that clip is not present in the document"
                    ),
                }));
            };
            if timeline_clip.id == args.clip_id {
                selected_clip_is_rendered = true;
            }
            let Some(asset_id) = asset_id else {
                // Titles are compositor-native overlays and do not require a
                // source file. Record the production title layer explicitly,
                // without inventing asset identity or availability fields.
                let kinewright_media::TimelineVisualLayer::Title(title_layer) = &layer else {
                    unreachable!("only title layers omit an asset id")
                };
                let ClipContent::Title(document_title) = &timeline_clip.content else {
                    return Ok(color_proof_error_result(ColorProofError::RenderFailed {
                        stage: "visual_layer_resolution",
                        message: format!(
                            "production visual resolver returned a title layer for track {track_id} clip {clip_id}, but the document clip is not a title"
                        ),
                    }));
                };
                if document_title != &title_layer.title {
                    return Ok(color_proof_error_result(ColorProofError::RenderFailed {
                        stage: "visual_layer_resolution",
                        message: format!(
                            "production visual resolver returned title parameters that differ from track {track_id} clip {clip_id}"
                        ),
                    }));
                }
                active_rendered_layers.push(serde_json::json!({
                    "track_id": track_id.0,
                    "clip_id": clip_id.0,
                    "content": "title",
                    "title": title_layer.title,
                    "effects": proof_effect_manifest(&title_layer.effects),
                    "color_nodes": proof_color_node_manifest(&title_layer.effects, &looks),
                    "transition": {
                        "alpha": title_layer.transition.alpha,
                        "fade_mix": title_layer.transition.fade_mix,
                        "fade_white": title_layer.transition.fade_white,
                    },
                    "legacy_stage_warnings": legacy_stage_warnings(timeline_clip),
                }));
                unsupported_layer_warnings.extend(Self::layer_compatibility_warnings(
                    track_id,
                    timeline_clip,
                    None,
                ));
                continue;
            };
            if timeline_clip.asset != asset_id {
                return Ok(color_proof_error_result(ColorProofError::RenderFailed {
                    stage: "visual_layer_resolution",
                    message: format!(
                        "production visual resolver mapped track {track_id} clip {clip_id} to asset {asset_id}, but the document clip references asset {}",
                        timeline_clip.asset
                    ),
                }));
            }
            let Some(timeline_asset) = document.asset(asset_id) else {
                return Ok(color_proof_error_result(ColorProofError::RenderFailed {
                    stage: "visual_layer_resolution",
                    message: format!(
                        "production visual resolver returned missing asset {asset_id} for track {track_id} clip {clip_id}"
                    ),
                }));
            };
            let availability = self.analysis.media_availability(timeline_asset);
            if !matches!(
                availability.kind,
                kinewright_core::MediaAvailabilityKind::OnlineVerified
                    | kinewright_core::MediaAvailabilityKind::OnlineUnverified
            ) {
                return Ok(color_proof_error_result(
                    ColorProofError::MediaUnavailable {
                        clip: timeline_clip.id,
                        asset: timeline_asset.id,
                        status: availability,
                    },
                ));
            }
            let kinewright_media::TimelineVisualLayer::Video(video_layer) = &layer else {
                unreachable!("only video layers include an asset id")
            };
            let content = match &timeline_clip.content {
                ClipContent::Media => "media",
                ClipContent::Freeze(_) => "freeze",
                ClipContent::Title(_) => {
                    return Ok(color_proof_error_result(ColorProofError::RenderFailed {
                        stage: "visual_layer_resolution",
                        message: format!(
                            "production visual resolver returned a source-backed layer for title clip {clip_id} on track {track_id}"
                        ),
                    }));
                }
            };
            // Every active layer is composited into the same BEFORE/AFTER
            // raster, so a non-selected layer's source profile is part of the
            // proof's claim and is classified with the same normative
            // assumption rather than left unreported.
            let (layer_source_status, layer_source_error) =
                active_layer_source_classification(&timeline_asset.color_description);
            if let Some(error) = layer_source_error
                && blocking_layer_source.is_none()
            {
                // The full warning list is only known once every layer has been
                // classified, so the refusal is assembled after the loop.
                blocking_layer_source = Some((timeline_clip.id, timeline_asset.id, error));
            }
            unsupported_layer_warnings.extend(Self::layer_compatibility_warnings(
                track_id,
                timeline_clip,
                Some(timeline_asset.id),
            ));
            active_rendered_layers.push(serde_json::json!({
                "track_id": track_id.0,
                "clip_id": clip_id.0,
                "content": content,
                "asset_id": timeline_asset.id.0,
                "source_frame": video_layer.source.source_at.0,
                "source_end": video_layer.source.source_end.0,
                "timeline_end": video_layer.source.timeline_end.0,
                "source": {
                    "raw_description": timeline_asset.color_description,
                    "provenance": timeline_asset.color_description.provenance,
                    "confidence_basis_points": timeline_asset.color_description.confidence_basis_points,
                    "status": layer_source_status,
                },
                "source_fingerprint": timeline_asset.source_fingerprint,
                "availability": availability,
                // `visual_layers_at` has already evaluated clip-local
                // automation at this exact project frame. Preserve the
                // serialized vector order and resolved primary values so the
                // production layer can be reproduced from the manifest.
                "effects": proof_effect_manifest(&video_layer.effects),
                "color_nodes": proof_color_node_manifest(&video_layer.effects, &looks),
                "transition": {
                    "alpha": video_layer.transition.alpha,
                    "fade_mix": video_layer.transition.fade_mix,
                    "fade_white": video_layer.transition.fade_white,
                },
                "legacy_stage_warnings": legacy_stage_warnings(timeline_clip),
            }));
            // Retain one manifest entry per rendered clip so per-clip legacy
            // warnings remain observable when an asset is overlaid more than
            // once.
            active_rendered_sources.push((track_id, timeline_clip, timeline_asset, availability));
        }
        // A proof whose composite includes an unsupported source cannot honestly
        // claim managed CC1 conformance, so it fails with the exact
        // asset/field/observed/allowed evidence instead of rendering. The
        // non-blocking layer warnings ride along: this error path is the only
        // place they can still be reported for this composite.
        if let Some((clip, asset, error)) = blocking_layer_source {
            return Ok(color_proof_error_result(
                ColorProofError::UnsupportedActiveLayerSource {
                    clip,
                    asset,
                    error,
                    layer_warnings: unsupported_layer_warnings,
                },
            ));
        }
        if !selected_clip_is_rendered {
            return Ok(color_proof_error_result(ColorProofError::RenderFailed {
                stage: "selected_visual_layer",
                message: format!(
                    "selected clip {} is not an active rendered visual layer at project frame {}; an overlapping or higher-priority clip may obscure it",
                    args.clip_id, args.timecode
                ),
            }));
        }

        // CC4 §8: the BEFORE cell of a stored-node proof is the same composite
        // with the node removed, so `bypass` can be asserted byte-identical to
        // `before` rather than merely assumed (CC4 §3.6).
        let scratch_document = |operations: &[Operation]| -> Result<Arc<Document>, String> {
            if operations.is_empty() {
                return Ok(Arc::clone(&document));
            }
            let mut candidate = (*document).clone();
            apply_batch(&mut candidate, operations).map_err(|error| error.to_string())?;
            Ok(Arc::new(candidate))
        };
        // CC5 §7: the scratch automation this proof had to remove to render the
        // variant it names, published in the manifest so the removal is a
        // stated fact rather than an invisible difference from the document.
        let mut cleared_keyframes: Vec<&'static str> = Vec::new();
        let clip_local = args
            .timecode
            .checked_sub(clip.timeline_start)
            .unwrap_or(TimeCode::ZERO);
        let (before_operations, after_operations) = match stored_node {
            None => (Vec::new(), plan.operations.clone()),
            Some((effect_id, _)) => {
                let remove = vec![Operation::RemoveEffect {
                    clip: args.clip_id,
                    effect: effect_id,
                }];
                // CC5 §7: `inside_only` is the document exactly as stored, and
                // `outside_only` is a scratch copy with `matte_invert` toggled,
                // so the two variants partition the raster.
                let after = match (args.matte_comparison, look_comparison) {
                    (Some(MatteComparison::OutsideOnly), _) => {
                        let stored_effect = document.clip(args.clip_id).and_then(|clip| {
                            clip.effects.iter().find(|effect| effect.id == effect_id)
                        });
                        // `matte_invert` is Hold-only but it *is* keyframable,
                        // and automation beats the stored static value at every
                        // frame from its first keyframe onward. So the value to
                        // complement is the one this frame actually renders,
                        // and the static write only lands once the curve is out
                        // of the way — otherwise the "outside" cell would
                        // silently render the inside and the manifest would say
                        // otherwise. The clear is emitted on the scratch copy
                        // only, and only when a curve exists, so a node without
                        // automation produces the byte-identical single
                        // operation it always did.
                        let rendered_invert = stored_effect
                            .and_then(|effect| {
                                effect.integer_parameter_at(MATTE_INVERT_PARAMETER, clip_local)
                            })
                            .map_or_else(
                                || {
                                    stored_effect
                                        .map(kinewright_core::MatteParams::from_effect)
                                        .is_some_and(|matte| matte.is_inverted())
                                },
                                |value| value != 0,
                            );
                        let keyframed = stored_effect.is_some_and(|effect| {
                            effect
                                .keyframes
                                .get(MATTE_INVERT_PARAMETER)
                                .is_some_and(|curve| !curve.keyframes.is_empty())
                        });
                        if keyframed {
                            cleared_keyframes.push(MATTE_INVERT_PARAMETER);
                        }
                        let mut operations = Vec::new();
                        if keyframed {
                            operations.push(Operation::ClearEffectKeyframes {
                                clip: args.clip_id,
                                effect: effect_id,
                                name: MATTE_INVERT_PARAMETER.to_owned(),
                            });
                        }
                        operations.push(Operation::SetEffectParam {
                            clip: args.clip_id,
                            effect: effect_id,
                            name: MATTE_INVERT_PARAMETER.to_owned(),
                            value: ParamValue::Integer(i64::from(!rendered_invert)),
                        });
                        operations
                    }
                    (Some(MatteComparison::Coverage | MatteComparison::InsideOnly), _) => {
                        Vec::new()
                    }
                    (None, LookComparison::Before) => remove.clone(),
                    (None, LookComparison::After) => Vec::new(),
                    (None, LookComparison::Bypass) => vec![Operation::SetEffectParam {
                        clip: args.clip_id,
                        effect: effect_id,
                        name: kinewright_core::COLOR_NODE_BYPASS_PARAMETER.to_owned(),
                        value: ParamValue::Integer(1),
                    }],
                };
                (remove, after)
            }
        };
        // A request that changes nothing produces no operations at all. Core
        // rejects an empty batch, and an identical BEFORE/AFTER is the honest
        // proof of a no-op request.
        let before_document = match scratch_document(&before_operations) {
            Ok(document) => document,
            Err(message) => {
                return Ok(color_proof_error_result(ColorProofError::RenderFailed {
                    stage: "before_document",
                    message,
                }));
            }
        };
        let after_document = match scratch_document(&after_operations) {
            Ok(document) => document,
            Err(message) => {
                return Ok(color_proof_error_result(ColorProofError::RenderFailed {
                    stage: "candidate_document",
                    message,
                }));
            }
        };
        let before = match self
            .analysis
            .monitor_proof_for_document(Arc::clone(&before_document), args.timecode)
        {
            Ok(proof) => proof,
            Err(error) => {
                // CC4 §2.3: an unpublished LUT asset is a typed refusal
                // naming the asset, not a prose render failure.
                return Ok(color_proof_error_result(
                    ColorProofError::from_proof_render_error(
                        "before",
                        error,
                        &looks,
                        stored_node.map(|(effect, _)| effect),
                    ),
                ));
            }
        };
        let after = match self
            .analysis
            .monitor_proof_for_document(Arc::clone(&after_document), args.timecode)
        {
            Ok(proof) => proof,
            Err(error) => {
                return Ok(color_proof_error_result(
                    ColorProofError::from_proof_render_error(
                        "after",
                        error,
                        &looks,
                        stored_node.map(|(effect, _)| effect),
                    ),
                ));
            }
        };
        if !before.metadata.full_resolution || !after.metadata.full_resolution {
            return Ok(color_proof_error_result(ColorProofError::InvalidImage {
                stage: "before_after",
                message: "managed monitor proof did not report full_resolution=true".to_owned(),
            }));
        }
        if before.metadata != after.metadata {
            return Ok(color_proof_error_result(ColorProofError::InvalidImage {
                stage: "before_after",
                message: format!(
                    "before/after renderer provenance differs: {:?} vs {:?}",
                    before.metadata, after.metadata
                ),
            }));
        }
        if before.image.width != document.resolution.0
            || before.image.height != document.resolution.1
            || after.image.width != document.resolution.0
            || after.image.height != document.resolution.1
        {
            return Ok(color_proof_error_result(ColorProofError::InvalidImage {
                stage: "before_after",
                message: format!(
                    "full-resolution proof raster must match document resolution {}x{}; before={}x{}, after={}x{}",
                    document.resolution.0,
                    document.resolution.1,
                    before.image.width,
                    before.image.height,
                    after.image.width,
                    after.image.height,
                ),
            }));
        }
        // CC5 §7: `coverage` replaces the AFTER cell with the §4.1 proof
        // image itself. It is rendered here, after the BEFORE/AFTER rasters
        // have been proved to match the document raster, so the coverage is
        // asserted to be the same size as the picture it describes.
        let mut after = after;
        let mut matte_coverage = None;
        if matches!(args.matte_comparison, Some(MatteComparison::Coverage))
            && let Some((effect_id, _)) = stored_node
        {
            let proof = match self.analysis.matte_proof_for_document(
                Arc::clone(&after_document),
                args.timecode,
                args.clip_id,
                effect_id,
            ) {
                Ok(proof) => proof,
                Err(error) => {
                    return Ok(color_proof_error_result(
                        ColorProofError::MatteProofUnavailable {
                            effect: effect_id,
                            message: error.to_string(),
                        },
                    ));
                }
            };
            if proof.coverage.width != before.image.width
                || proof.coverage.height != before.image.height
            {
                return Ok(color_proof_error_result(ColorProofError::InvalidImage {
                    stage: "matte_coverage",
                    message: format!(
                        "coverage raster {}x{} does not match the proof raster {}x{}",
                        proof.coverage.width,
                        proof.coverage.height,
                        before.image.width,
                        before.image.height,
                    ),
                }));
            }
            matte_coverage = kinewright_core::matte_coverage_statistics(&proof.coverage)
                .ok()
                .map(|statistics| {
                    serde_json::json!({
                        "statistics": statistics,
                        "covered_pixel_count": statistics.covered_pixel_count,
                        "matte_threshold": kinewright_core::MATTE_SCOPE_THRESHOLD,
                        "coverage_encoding": proof.metadata.coverage_encoding,
                        "coverage_scale": proof.metadata.coverage_scale,
                        "raster_aspect_millionths": proof.metadata.raster_aspect_millionths,
                    })
                });
            after.image = proof.coverage;
        }
        // CC4 §8: the manifest *asserts* that the bypass variant is the
        // byte-identical twin of the node-removed variant. A difference means
        // a bypassed node still contributed something, so the proof is refused
        // with both hashes and both rasters rather than published with a
        // `bypass_matches_absent: false` footnote nobody has to read.
        let bypass_matches_absent = match (look_comparison, stored_node) {
            (LookComparison::Bypass, Some((effect_id, _))) => {
                let absent = kinewright_media::sha256_bytes(&before.image.pixels);
                let bypassed = kinewright_media::sha256_bytes(&after.image.pixels);
                if absent != bypassed
                    || before.image.width != after.image.width
                    || before.image.height != after.image.height
                {
                    return Ok(color_proof_error_result(
                        ColorProofError::BypassNotLossless {
                            effect: effect_id,
                            absent_rgba8_pixels_sha256: absent,
                            bypass_rgba8_pixels_sha256: bypassed,
                            absent_raster: (before.image.width, before.image.height),
                            bypass_raster: (after.image.width, after.image.height),
                        },
                    ));
                }
                Some(true)
            }
            _ => None,
        };
        let objective = match color_proof_objective(&before.image, &after.image) {
            Ok(objective) => objective,
            Err(message) => {
                return Ok(color_proof_error_result(ColorProofError::InvalidImage {
                    stage: "before_after",
                    message,
                }));
            }
        };
        let sheet = match compose_contact_sheet(&[before.image.clone(), after.image.clone()]) {
            Ok(sheet) => sheet,
            Err(error) => {
                return Ok(color_proof_error_result(ColorProofError::RenderFailed {
                    stage: "before_after_composition",
                    message: error.to_string(),
                }));
            }
        };
        let png = match encode_png(&sheet) {
            Ok(png) => png,
            Err(error) => {
                return Ok(color_proof_error_result(ColorProofError::RenderFailed {
                    stage: "png_encoding",
                    message: error.to_string(),
                }));
            }
        };
        let before_png = match encode_png(&before.image) {
            Ok(png) => png,
            Err(error) => {
                return Ok(color_proof_error_result(ColorProofError::RenderFailed {
                    stage: "before_png_encoding",
                    message: error.to_string(),
                }));
            }
        };
        let after_png = match encode_png(&after.image) {
            Ok(png) => png,
            Err(error) => {
                return Ok(color_proof_error_result(ColorProofError::RenderFailed {
                    stage: "after_png_encoding",
                    message: error.to_string(),
                }));
            }
        };
        let hashes = serde_json::json!({
            "before_rgba8_pixels_sha256": kinewright_media::sha256_bytes(&before.image.pixels),
            "after_rgba8_pixels_sha256": kinewright_media::sha256_bytes(&after.image.pixels),
            "before_png_bytes_sha256": kinewright_media::sha256_bytes(&before_png),
            "after_png_bytes_sha256": kinewright_media::sha256_bytes(&after_png),
            "contact_sheet_rgba8_pixels_sha256": kinewright_media::sha256_bytes(&sheet.pixels),
            "contact_sheet_png_bytes_sha256": kinewright_media::sha256_bytes(&png),
        });
        let operations = serde_json::to_value(&plan.operations)
            .map_err(|error| McpError::internal_error(error.to_string(), None))?;
        let profile_assumption = plan.profile_assumption.map(|_| {
            serde_json::json!({
                "selected": "d65",
                "source": if args.profile_assumption.is_some() {
                    "explicit"
                } else {
                    "application_profile_assumption"
                },
            })
        });
        let clip_local_frame = args
            .timecode
            .checked_sub(clip.timeline_start)
            .unwrap_or(TimeCode::ZERO);
        // CC5 §7, hoisted out of the manifest literal so the `json!` macro
        // stays inside its recursion budget.
        let matte_comparison_manifest = args.matte_comparison.map(|variant| {
            serde_json::json!({
                "variant": variant.as_str(),
                "effect_id": stored_node.map(|(effect_id, _)| effect_id.0),
                "kind": stored_node.map(|(_, kind)| kind.effect_name()),
                "after_cell": match variant {
                    MatteComparison::Coverage => "the CC5 §4.1 coverage image, R = G = B = round(255 * m), alpha 255",
                    MatteComparison::InsideOnly => "the document as stored: the correction applies inside the matte and nowhere else",
                    MatteComparison::OutsideOnly => "a scratch copy with matte_invert toggled: the correction applies outside the matte and nowhere else",
                },
                "after_operations": after_operations,
                // CC5 §7: `matte_invert` is Hold-only but keyframable, and a
                // static write under an existing curve is dead. `outside_only`
                // therefore clears that curve on the scratch copy, and names it
                // here; empty for every other variant and for a node with no
                // `matte_invert` automation.
                "cleared_keyframes": cleared_keyframes,
                "coverage": matte_coverage,
            })
        });
        let manifest = serde_json::json!({
            "timeline_revision": actual_revision.0,
            "clip_id": args.clip_id.0,
            "asset_id": asset.id.0,
            "active_rendered_layers": active_rendered_layers,
            "unsupported_layer_warnings": unsupported_layer_warnings,
            "active_rendered_sources": active_rendered_sources.iter().map(|(track_id, active_clip, active_asset, availability)| {
                serde_json::json!({
                    "track_id": track_id.0,
                    "clip_id": active_clip.id.0,
                    "content": match &active_clip.content {
                        ClipContent::Media => "media",
                        ClipContent::Freeze(_) => "freeze",
                        ClipContent::Title(_) => "title",
                    },
                    "asset_id": active_asset.id.0,
                    "source": {
                        "raw_description": active_asset.color_description,
                        "provenance": active_asset.color_description.provenance,
                        "confidence_basis_points": active_asset.color_description.confidence_basis_points,
                    },
                    "source_fingerprint": active_asset.source_fingerprint,
                    "availability": availability,
                    "legacy_stage_warnings": legacy_stage_warnings(active_clip),
                })
            }).collect::<Vec<_>>(),
            "project_frame": args.timecode.0,
            "clip_local_frame": clip_local_frame.0,
            "source_profile": plan.source_profile.id(),
            "source": {
                "raw_description": asset.color_description,
                "provenance": asset.color_description.provenance,
                "confidence_basis_points": asset.color_description.confidence_basis_points,
                "profile_assumption": profile_assumption,
            },
            "profile_assumption": profile_assumption,
            "render_kind": before.metadata.render_kind,
            "renderer": "analysis.monitor_proof_for_document",
            "backend": before.metadata.backend,
            "adapter": before.metadata.adapter,
            "backend_provenance": {
                "backend": before.metadata.backend,
                "adapter": before.metadata.adapter,
                "software_fallback": before.metadata.software_fallback,
            },
            "software_fallback": before.metadata.software_fallback,
            "gpu_claim": before.metadata.gpu_claim,
            "full_resolution": before.metadata.full_resolution,
            "cpu_reference": false,
            "decoded_delivery": false,
            "ordered_stage_names": CC1_STAGE_NAMES,
            "legacy_stage_warnings": legacy_stage_warnings(clip),
            "color_context": {
                "pipeline_state": document.color_context.pipeline_state,
                "working": document.color_context.working,
                "monitoring": document.color_context.monitoring,
                "delivery": document.color_context.delivery,
            },
            "formats": {
                "input": {
                    "bit_depth": asset.color_description.bit_depth,
                    "range": asset.color_description.range,
                    "raster": asset.resolution,
                },
                "working": {
                    "bit_depth": document.color_context.working.bit_depth,
                    "range": document.color_context.working.range,
                },
                "monitoring": {
                    "bit_depth": document.color_context.monitoring.bit_depth,
                    "range": document.color_context.monitoring.range,
                },
                "delivery": {
                    "bit_depth": document.color_context.delivery.bit_depth,
                    "range": document.color_context.delivery.range,
                },
                "output": {
                    "bit_depth": "rgba8",
                    "range": document.color_context.monitoring.range,
                    "raster": [before.image.width, before.image.height],
                },
            },
            "sampling_region": {
                "project_frame": args.timecode.0,
                "clip_id": args.clip_id.0,
                "clip_local_frame": clip_local_frame.0,
            },
            "primary_correction": {
                "requested_parameters": plan.requested_parameters,
                "resolved_parameters": plan.resolved_parameters,
            },
            // CC4 §8: which variant the AFTER cell actually rendered, and the
            // exact scratch operations each cell was rendered from.
            "look_comparison": stored_node.map(|(effect_id, kind)| serde_json::json!({
                "effect_id": effect_id.0,
                "kind": kind.effect_name(),
                "role": kind.role(),
                "color_stage": kind.stage().as_str(),
                "variant": look_comparison.as_str(),
                "before_variant": "absent",
                "bypass_matches_absent": bypass_matches_absent,
                "before_operations": before_operations,
                "after_operations": after_operations,
            })),
            "operations": operations,
            "evidence_only": true,
            "applied": false,
            "cells": [
                {
                    "cell": "before",
                    "label": "BEFORE",
                    "index": 0,
                    "x": 0,
                    "y": 0,
                    "width": before.image.width,
                    "height": before.image.height,
                },
                {
                    "cell": "after",
                    "label": "AFTER",
                    "index": 1,
                    "x": before.image.width.saturating_add(STORYBOARD_GUTTER),
                    "y": 0,
                    "width": after.image.width,
                    "height": after.image.height,
                },
            ],
            "sheet": {"width": sheet.width, "height": sheet.height},
            "hashes": hashes,
            "objective": objective,
            "next": "Review the mapped BEFORE/AFTER cells and exact unapplied operations; submit through prepare_edit_plan at the same revision only if the edit is requested.",
        });
        // CC5 §7: inserted rather than written into the literal above, which is
        // already at the `json!` macro's recursion budget. Absent entirely when
        // no matte variant was requested, so a CC4 manifest is byte-unchanged.
        let mut manifest = manifest;
        if let Some(matte_comparison) = matte_comparison_manifest
            && let Some(object) = manifest.as_object_mut()
        {
            object.insert("matte_comparison".to_owned(), matte_comparison);
        }
        let mut result = CallToolResult::success(vec![
            ContentBlock::text(format!(
                "CC1 colour proof clip={} asset={} project_frame={} revision={} BEFORE|AFTER",
                args.clip_id, asset.id, args.timecode, actual_revision
            )),
            ContentBlock::image(BASE64.encode(png), "image/png"),
        ]);
        result.structured_content = Some(manifest);
        Ok(result)
    }

    /// Resolve a production visual-layer identity back to its exact document
    /// clip. The media resolver owns interval, transition, freeze-frame, and
    /// overlap semantics; this lookup only joins its stable ids to metadata
    /// needed by the proof manifest.
    fn document_clip_on_track(
        document: &Document,
        track_id: TrackId,
        clip_id: ClipId,
    ) -> Option<&Clip> {
        document
            .tracks
            .iter()
            .find(|track| track.id == track_id)?
            .clips
            .iter()
            .find(|clip| clip.id == clip_id)
    }

    /// Every non-blocking reason one active proof layer falls outside the
    /// managed CC1 claim: each post-primary compatibility stage on the layer's
    /// effect chain.
    ///
    /// This covers non-selected layers too, so a proof can never present an
    /// unqualified managed claim for a composite that contains one.
    ///
    /// A blocking source profile is deliberately not reported here. It refuses
    /// the proof outright, so it is carried by the
    /// `active_layer_needs_color_override` error rather than by a warning that
    /// no successful response could ever contain.
    fn layer_compatibility_warnings(
        track_id: TrackId,
        clip: &Clip,
        asset: Option<AssetId>,
    ) -> Vec<serde_json::Value> {
        let mut warnings = Vec::new();
        warnings.extend(legacy_stage_warnings(clip).into_iter().map(|warning| {
            serde_json::json!({
                "track_id": track_id.0,
                "clip_id": clip.id.0,
                "asset_id": asset.map_or(serde_json::Value::Null, |asset| {
                    serde_json::json!(asset.0)
                }),
                "code": warning["code"].clone(),
                "blocking": false,
                "message": warning["message"].clone(),
                "effect_id": warning["effect_id"].clone(),
                "effect_index": warning["effect_index"].clone(),
                "name": warning["name"].clone(),
            })
        }));
        warnings
    }
}

/// Ordered production effect manifest for one resolved visual layer.
///
/// `visual_layers_at` evaluates clip-local automation before handing effects
/// to the compositor, so the returned parameters are the values actually
/// rendered at that frame. The shared `color_status` helper owns the shape so
/// the proof manifest and `get_color_context` can never disagree.
fn proof_effect_manifest(effects: &[Effect]) -> Vec<serde_json::Value> {
    effect_chain_manifest(effects)
}

/// Keep the ordered CC3 colour-node stack aligned with `get_color_context`
/// while retaining the complete ordered effect chain above. This is
/// intentionally derived from the same frame-evaluated effects, never from the
/// raw clip vector, so bypass, activity, and resolved curve points describe the
/// frame that was actually rendered.
fn proof_color_node_manifest(
    effects: &[Effect],
    looks: &LookAssetContext,
) -> Vec<serde_json::Value> {
    color_node_manifest(effects, looks)
}

#[allow(clippy::needless_pass_by_value)]
fn color_proof_error_result(error: ColorProofError) -> CallToolResult {
    error_structured(
        format!("CC1 colour proof rejected: {error}"),
        serde_json::json!({
            "code": error.code(),
            "message": error.to_string(),
            "details": error.details(),
            "evidence_only": true,
            "applied": false,
        }),
    )
}

fn color_proof_objective(
    before: &kinewright_core::RgbaImage,
    after: &kinewright_core::RgbaImage,
) -> Result<serde_json::Value, String> {
    let expected_len = usize::try_from(before.width)
        .ok()
        .and_then(|width| {
            usize::try_from(before.height)
                .ok()
                .and_then(|height| width.checked_mul(height))
        })
        .and_then(|pixels| pixels.checked_mul(4))
        .ok_or_else(|| "before raster dimensions overflowed".to_owned())?;
    if before.width != after.width || before.height != after.height {
        return Err(format!(
            "before raster is {}x{}, after raster is {}x{}",
            before.width, before.height, after.width, after.height
        ));
    }
    if before.pixels.len() != expected_len || after.pixels.len() != expected_len {
        return Err(format!(
            "RGBA8 raster length does not match {}x{} dimensions",
            before.width, before.height
        ));
    }
    let mut deltas = Vec::with_capacity(expected_len / 4 * 3);
    let mut before_clipped = 0_u128;
    let mut after_clipped = 0_u128;
    let mut channel_count = 0_u128;
    let mut delta_sum = 0_u128;
    for (before_pixel, after_pixel) in before
        .pixels
        .as_chunks::<4>()
        .0
        .iter()
        .zip(after.pixels.as_chunks::<4>().0.iter())
    {
        for channel in 0..3 {
            let before_channel = before_pixel[channel];
            let after_channel = after_pixel[channel];
            if before_channel == 0 || before_channel == u8::MAX {
                before_clipped = before_clipped.saturating_add(1);
            }
            if after_channel == 0 || after_channel == u8::MAX {
                after_clipped = after_clipped.saturating_add(1);
            }
            let delta = before_channel.abs_diff(after_channel);
            deltas.push(delta);
            delta_sum = delta_sum.saturating_add(u128::from(delta));
            channel_count = channel_count.saturating_add(1);
        }
    }
    if deltas.is_empty() {
        return Err("RGBA8 raster contains no RGB channels".to_owned());
    }
    deltas.sort_unstable();
    let p99_index = deltas
        .len()
        .saturating_mul(99)
        .div_ceil(100)
        .saturating_sub(1);
    let denominator = channel_count.saturating_mul(u128::from(u8::MAX));
    let mean_basis_points = delta_sum
        .saturating_mul(10_000)
        .saturating_add(denominator / 2)
        / denominator;
    let clipping_basis_points = |count: u128| {
        let rounded = count
            .saturating_mul(10_000)
            .saturating_add(channel_count / 2)
            / channel_count;
        u16::try_from(rounded).unwrap_or(u16::MAX).min(10_000)
    };
    let mean_milli_code_values = delta_sum
        .saturating_mul(1_000)
        .saturating_add(channel_count / 2)
        / channel_count;
    Ok(serde_json::json!({
        "max_channel_delta_code_values": deltas.last().copied().unwrap_or_default(),
        "p99_channel_delta_code_values": deltas[p99_index],
        "mean_channel_delta_milli_code_values": u32::try_from(mean_milli_code_values)
            .unwrap_or(u32::MAX),
        "mean_normalized_delta_basis_points": u16::try_from(mean_basis_points)
            .unwrap_or(u16::MAX)
            .min(10_000),
        "clipping_basis_points": {
            "before": clipping_basis_points(before_clipped),
            "after": clipping_basis_points(after_clipped),
            "definition": "RGB channels equal to final RGBA8 code 0 or 255",
        },
    }))
}
