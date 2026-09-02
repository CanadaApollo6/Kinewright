//! Source monitor, source/program edit planning, shot boards, and faceted media search.

use super::storyboards::{compose_contact_sheet, storyboard_sample_frames};
use super::*;

impl KinewrightMcp {
    #[allow(clippy::too_many_lines)]
    pub(super) fn source_storyboard(
        &self,
        args: &SourceStoryboardArgs,
    ) -> Result<CallToolResult, McpError> {
        let (revision, document) = self.snapshot()?;
        let Some(asset) = document.asset(args.asset_id).cloned() else {
            return Ok(error_text(format!(
                "asset {} does not exist",
                args.asset_id
            )));
        };
        if !asset.kind.supports(TrackKind::Video) {
            return Ok(error_text(format!(
                "asset {} is not a video asset",
                asset.id
            )));
        }
        if let Some(error) = self.source_availability_error(&asset, "source storyboard") {
            return Ok(error);
        }

        let source_in = args
            .range
            .as_ref()
            .map_or(TimeCode::ZERO, |range| range.start);
        let source_out = args
            .range
            .as_ref()
            .map_or(asset.duration, |range| range.end);
        if source_in < TimeCode::ZERO || source_out > asset.duration || source_out <= source_in {
            return Ok(error_text(format!(
                "source storyboard range {source_in}..{source_out} is outside asset {} range 0..{}",
                asset.id, asset.duration
            )));
        }

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
        let source_range = source_in..source_out;
        let duration = match map_source_range_to_project(source_range.clone(), asset.fps, asset.fps)
        {
            Ok(duration) => duration,
            Err(error) => return Ok(error_text(error.to_string())),
        };
        let temporary = Arc::new(Document {
            tracks: vec![Track {
                id: TrackId(1),
                kind: TrackKind::Video,
                sync_lock: true,
                clips: vec![Clip {
                    id: ClipId(1),
                    asset: asset.id,
                    source_range,
                    content: ClipContent::Media,
                    timeline_start: TimeCode::ZERO,
                    effects: Vec::new(),
                    transition_in: None,
                    link: None,
                    audio_gain_tenth_db: 0,
                    audio_fade_in_frames: TimeCode::ZERO,
                    audio_fade_out_frames: TimeCode::ZERO,
                    speed_percent: 100,
                }],
            }],
            media_pool: vec![asset.clone()],
            fps: asset.fps,
            resolution: asset.resolution.unwrap_or((1_920, 1_080)),
            duration,
            ..Document::default()
        });

        let frames = storyboard_sample_frames(&(TimeCode::ZERO..duration), frame_count);
        let mut images = Vec::with_capacity(frames.len());
        for frame in &frames {
            match self
                .analysis
                .thumbnail_for_document(Arc::clone(&temporary), *frame, max_width)
            {
                Ok(image) => images.push(image),
                Err(error) => return Ok(error_text(error.to_string())),
            }
        }
        let sheet = compose_contact_sheet(&images)?;
        let png = encode_png(&sheet)?;
        let source_range_value = serde_json::json!({
            "start": source_in.0,
            "end": source_out.0,
        });
        let cells = frames
            .iter()
            .enumerate()
            .map(|(index, frame)| {
                let source_frame = source_in
                    .checked_add(*frame)
                    .expect("validated source storyboard frame cannot overflow");
                serde_json::json!({
                    "cell": index + 1,
                    "asset_id": asset.id.0,
                    "source_frame": source_frame.0,
                    "source_range": source_range_value.clone(),
                })
            })
            .collect::<Vec<_>>();
        let manifest = serde_json::json!({
            "timeline_revision": revision.0,
            "asset_id": asset.id.0,
            "source_range": source_range_value,
            "cells": cells,
            "sheet": {"width": sheet.width, "height": sheet.height},
        });
        let mut result = CallToolResult::success(vec![
            ContentBlock::text(format!(
                "source storyboard asset={} range={}..{} cells={}\n{}",
                asset.id,
                source_in,
                source_out,
                frames.len(),
                manifest
            )),
            ContentBlock::image(BASE64.encode(png), "image/png"),
        ]);
        result.structured_content = Some(manifest);
        Ok(result)
    }

    /// Prepare one explicit source/program patch against one observed
    /// timeline revision. Core owns the compound operation's exact
    /// three-point derivation, route validation, insert/overwrite semantics,
    /// and linked A/V construction; this boundary only adds typed agent
    /// routing, revision gating, and inspectable evidence around it.
    #[allow(clippy::too_many_lines)]
    pub(super) fn source_program_edit_plan(
        &self,
        args: &SourceProgramEditArgs,
    ) -> Result<CallToolResult, McpError> {
        let (revision, document) = self.snapshot()?;
        if args.expected_revision != revision {
            return Ok(revision_conflict_text(args.expected_revision, revision));
        }

        let Some(asset) = document.asset(args.asset).cloned() else {
            return Ok(error_structured(
                format!("asset {} does not exist", args.asset),
                serde_json::json!({
                    "code": "missing_asset",
                    "asset_id": args.asset.0,
                    "timeline_revision": revision.0,
                }),
            ));
        };
        if let Some(error) = self.source_availability_error(&asset, "source program edit") {
            return Ok(error);
        }
        if args.video_track.is_none() && args.audio_track.is_none() {
            return Ok(error_structured(
                "source program edit requires at least one explicit destination",
                serde_json::json!({
                    "code": "empty_source_patch",
                    "asset_id": asset.id.0,
                    "timeline_revision": revision.0,
                    "video_track": serde_json::Value::Null,
                    "audio_track": serde_json::Value::Null,
                }),
            ));
        }
        if let (Some(video_track), Some(audio_track)) = (args.video_track, args.audio_track)
            && video_track == audio_track
        {
            let track = video_track;
            return Ok(error_structured(
                format!("source program edit targets track {track} more than once"),
                serde_json::json!({
                    "code": "duplicate_source_patch_track",
                    "track_id": track.0,
                    "timeline_revision": revision.0,
                }),
            ));
        }

        for (component, requested, expected_kind) in [
            ("video", args.video_track, TrackKind::Video),
            ("audio", args.audio_track, TrackKind::Audio),
        ] {
            let Some(track_id) = requested else {
                continue;
            };
            if !asset.kind.supports(expected_kind) {
                return Ok(error_structured(
                    format!(
                        "asset {} has no {component} component for destination track {track_id}",
                        asset.id
                    ),
                    serde_json::json!({
                        "code": "invalid_source_patch_route_kind",
                        "component": component,
                        "asset_kind": asset.kind,
                        "expected_track_kind": expected_kind,
                        "track_id": track_id.0,
                        "timeline_revision": revision.0,
                    }),
                ));
            }
            let Some(track) = document.tracks.iter().find(|track| track.id == track_id) else {
                return Ok(error_structured(
                    format!("destination track {track_id} does not exist"),
                    serde_json::json!({
                        "code": "missing_source_patch_track",
                        "component": component,
                        "track_id": track_id.0,
                        "timeline_revision": revision.0,
                    }),
                ));
            };
            if track.kind != expected_kind {
                return Ok(error_structured(
                    format!(
                        "{component} source route requires a {expected_kind:?} track, got {:?} track {track_id}",
                        track.kind
                    ),
                    serde_json::json!({
                        "code": "invalid_source_patch_route_kind",
                        "component": component,
                        "expected_track_kind": expected_kind,
                        "actual_track_kind": track.kind,
                        "track_id": track_id.0,
                        "timeline_revision": revision.0,
                    }),
                ));
            }
        }

        let operation = Operation::PatchedThreePointEdit {
            asset: asset.id,
            source_in: args.source_in,
            source_out: args.source_out,
            timeline_in: args.timeline_in,
            timeline_out: args.timeline_out,
            mode: args.mode,
            video_track: args.video_track,
            audio_track: args.audio_track,
        };
        // Resolve the derived range on an isolated, clip-free copy. This is
        // deliberately separate from the actual preview: overwrite may
        // remove the highest existing clip id and Core is allowed to reuse
        // that id for the replacement. Matching by source/timeline semantics
        // therefore remains correct where an id-only before/after diff would
        // lose the new clip.
        let mut range_document = document.as_ref().clone();
        for track in &mut range_document.tracks {
            track.clips.clear();
        }
        range_document.duration = TimeCode::ZERO;
        if let Err(error) = operation.apply(&mut range_document) {
            return Ok(error_structured(
                format!("source program edit is invalid: {error}"),
                serde_json::json!({
                    "code": "invalid_source_program_edit",
                    "asset_id": asset.id.0,
                    "timeline_revision": revision.0,
                    "mode": args.mode,
                    "video_track": args.video_track.map(|track| track.0),
                    "audio_track": args.audio_track.map(|track| track.0),
                    "error": error.to_string(),
                }),
            ));
        }
        let expected_clip = args
            .video_track
            .or(args.audio_track)
            .and_then(|track_id| {
                range_document
                    .tracks
                    .iter()
                    .find(|track| track.id == track_id)
                    .and_then(|track| track.clips.iter().find(|clip| clip.asset == asset.id))
            })
            .ok_or_else(|| {
                McpError::internal_error(
                    "patched source program range resolution produced no route clip",
                    None,
                )
            })?;
        let expected_source = expected_clip.source_range.clone();
        let expected_timeline_start = expected_clip.timeline_start;
        let mut projected = document.as_ref().clone();
        if let Err(error) = operation.apply(&mut projected) {
            return Ok(error_structured(
                format!("source program edit is invalid: {error}"),
                serde_json::json!({
                    "code": "invalid_source_program_edit",
                    "asset_id": asset.id.0,
                    "timeline_revision": revision.0,
                    "mode": args.mode,
                    "video_track": args.video_track.map(|track| track.0),
                    "audio_track": args.audio_track.map(|track| track.0),
                    "error": error.to_string(),
                }),
            ));
        }

        let mut routed_clips = BTreeMap::new();
        for (component, requested) in [("video", args.video_track), ("audio", args.audio_track)] {
            let Some(track_id) = requested else {
                continue;
            };
            let Some(clip) = projected
                .tracks
                .iter()
                .find(|track| track.id == track_id)
                .and_then(|track| {
                    track.clips.iter().find(|clip| {
                        clip.asset == asset.id
                            && clip.source_range == expected_source
                            && clip.timeline_start == expected_timeline_start
                    })
                })
            else {
                return Err(McpError::internal_error(
                    format!(
                        "patched source program operation did not produce its {component} route"
                    ),
                    None,
                ));
            };
            routed_clips.insert(component, clip.clone());
        }

        let first_clip = routed_clips
            .values()
            .next()
            .expect("at least one route was validated");
        let duration = projected
            .clip_duration(first_clip)
            .map_err(|error| McpError::internal_error(error.to_string(), None))?;
        let timeline_out = first_clip
            .timeline_start
            .checked_add(duration)
            .ok_or_else(|| McpError::internal_error("timeline range overflowed", None))?;
        for clip in routed_clips.values() {
            if clip.source_range != first_clip.source_range
                || clip.timeline_start != first_clip.timeline_start
            {
                return Err(McpError::internal_error(
                    "patched source program routes are not aligned",
                    None,
                ));
            }
        }
        let linked = routed_clips.len() > 1
            && routed_clips
                .values()
                .map(|clip| clip.link)
                .collect::<BTreeSet<_>>()
                .len()
                == 1
            && routed_clips.values().all(|clip| clip.link.is_some());
        if routed_clips.len() > 1 && !linked {
            return Err(McpError::internal_error(
                "patched source program routes are not linked",
                None,
            ));
        }

        let plan = match self.prepare_operations(revision, &document, vec![operation]) {
            Ok(plan) => plan,
            Err(error) => {
                return Ok(error_structured(
                    format!("source program edit could not be prepared: {error}"),
                    serde_json::json!({
                        "code": "invalid_source_program_edit",
                        "asset_id": asset.id.0,
                        "timeline_revision": revision.0,
                        "error": error,
                    }),
                ));
            }
        };

        let routed = routed_clips
            .iter()
            .map(|(component, clip)| {
                (
                    (*component).to_owned(),
                    serde_json::json!({
                        "track_id": if *component == "video" { args.video_track } else { args.audio_track },
                        "clip_id": clip.id.0,
                        "link_id": clip.link.map(|link| link.0),
                    }),
                )
            })
            .collect::<serde_json::Map<_, _>>();
        let structured = serde_json::json!({
            "timeline_revision": revision.0,
            "asset_id": asset.id.0,
            "mode": args.mode,
            "source_range": {
                "start": first_clip.source_range.start.0,
                "end": first_clip.source_range.end.0,
            },
            "timeline_range": {
                "start": first_clip.timeline_start.0,
                "end": timeline_out.0,
            },
            "destinations": routed,
            "linked": linked,
            "prepared_edit_plan": {
                "plan_id": plan.id,
                "expected_revision": revision,
                "preview": plan.preview,
            },
        });
        Ok(success_structured(
            format!(
                "prepared source program {} edit for asset {} as plan {}; inspect the preview, then commit it at timeline revision {revision}",
                match args.mode {
                    ThreePointMode::Insert => "insert",
                    ThreePointMode::Overwrite => "overwrite",
                },
                asset.id,
                plan.id,
            ),
            structured,
        ))
    }

    /// Return source-monitor candidates derived from cached scene boundaries.
    ///
    /// This deliberately builds an isolated, throwaway document for thumbnail
    /// rendering. It is an inspector: no Core command, prepared plan, or
    /// playback document is changed.
    #[allow(clippy::too_many_lines)]
    pub(super) fn source_shot_board(
        &self,
        args: &SourceShotBoardArgs,
    ) -> Result<CallToolResult, McpError> {
        let (revision, document) = self.snapshot()?;
        let Some(asset) = document.asset(args.asset_id).cloned() else {
            return Ok(error_text(format!(
                "asset {} does not exist",
                args.asset_id
            )));
        };
        if !asset.kind.supports(TrackKind::Video) {
            return Ok(error_text(format!(
                "asset {} is not a video asset",
                asset.id
            )));
        }
        if let Some(error) = self.source_availability_error(&asset, "source shot board") {
            return Ok(error);
        }

        let source_in = args
            .range
            .as_ref()
            .map_or(TimeCode::ZERO, |range| range.start);
        let source_out = args
            .range
            .as_ref()
            .map_or(asset.duration, |range| range.end);
        if source_in < TimeCode::ZERO || source_out > asset.duration || source_out <= source_in {
            return Ok(error_text(format!(
                "source shot board range {source_in}..{source_out} is outside asset {} range 0..{}",
                asset.id, asset.duration
            )));
        }
        let candidate_selection = args.candidate_selection.unwrap_or_default();
        if candidate_selection == ShotBoardCandidateSelection::Coverage
            && args.candidate_offset.is_some()
        {
            return Ok(error_text(
                "candidate_offset is only supported when candidate_selection is `page`; omit it when using `coverage`",
            ));
        }
        let candidate_count = args
            .candidate_count
            .unwrap_or(SHOT_BOARD_DEFAULT_CANDIDATES);
        if !(1..=SHOT_BOARD_MAX_CANDIDATES).contains(&candidate_count) {
            return Ok(error_text(format!(
                "candidate_count must be in 1..={SHOT_BOARD_MAX_CANDIDATES}"
            )));
        }
        let max_width = args.max_width.unwrap_or(STORYBOARD_DEFAULT_CELL_WIDTH);
        if !(64..=THUMBNAIL_MAX_WIDTH).contains(&max_width) {
            return Ok(error_text(format!(
                "max_width must be in 64..={THUMBNAIL_MAX_WIDTH}"
            )));
        }
        if let Some(minimum_duration_frames) = args.minimum_duration_frames
            && minimum_duration_frames.0 < 1
        {
            return Ok(error_text(
                "minimum_duration_frames must be at least 1 when provided",
            ));
        }
        let minimum_confidence_basis_points = args
            .minimum_confidence_basis_points
            .unwrap_or(DEFAULT_SCENE_CONFIDENCE_BASIS_POINTS);
        if minimum_confidence_basis_points > 10_000 {
            return Ok(error_text(
                "minimum_confidence_basis_points must be in 0..=10000",
            ));
        }

        let mut status = self.analysis.scene_status(&asset);
        if status == SceneStatus::NotRequested {
            self.analysis.request_scene_detection(asset.clone());
            status = self.analysis.scene_status(&asset);
        }
        let scenes = match status {
            SceneStatus::Ready(scenes) => scenes,
            SceneStatus::NotRequested
            | SceneStatus::Queued
            | SceneStatus::Hashing
            | SceneStatus::Analyzing => {
                let status = match status {
                    SceneStatus::NotRequested => "requested",
                    SceneStatus::Queued => "queued",
                    SceneStatus::Hashing => "hashing",
                    SceneStatus::Analyzing => "analyzing",
                    _ => unreachable!(),
                };
                let manifest = serde_json::json!({
                    "timeline_revision": revision.0,
                    "asset_id": asset.id.0,
                    "source_range": {"start": source_in.0, "end": source_out.0},
                    "status": "pending",
                    "analysis_status": status,
                    "scene_confidence_threshold_basis_points": minimum_confidence_basis_points,
                    "minimum_duration_frames": args.minimum_duration_frames.map(|duration| duration.0),
                    "candidate_selection": candidate_selection.as_str(),
                    "candidate_offset": (candidate_selection == ShotBoardCandidateSelection::Page).then(|| args.candidate_offset.unwrap_or(0)),
                    "requested_candidate_count": candidate_count,
                    "message": "scene analysis is pending; call get_source_shot_board again when it is ready",
                });
                let mut result = success_text(manifest.to_string());
                result.structured_content = Some(manifest);
                return Ok(result);
            }
            SceneStatus::NoVideo => {
                return Ok(error_text(format!(
                    "asset {} has no decodable video stream",
                    asset.id
                )));
            }
            SceneStatus::Cancelled => {
                return Ok(error_text(format!(
                    "scene analysis for asset {} was cancelled; request it again",
                    asset.id
                )));
            }
            SceneStatus::Failed(message) => {
                return Ok(error_text(format!(
                    "scene analysis for asset {} failed: {message}",
                    asset.id
                )));
            }
        };

        let mut cuts = BTreeMap::<TimeCode, u16>::new();
        for change in &scenes.changes {
            if change.confidence_basis_points >= minimum_confidence_basis_points
                && change.source_frame > source_in
                && change.source_frame < source_out
            {
                cuts.entry(change.source_frame)
                    .and_modify(|confidence| {
                        *confidence = (*confidence).max(change.confidence_basis_points);
                    })
                    .or_insert(change.confidence_basis_points);
            }
        }
        let boundaries = std::iter::once(source_in)
            .chain(cuts.keys().copied())
            .chain(std::iter::once(source_out))
            .collect::<Vec<_>>();
        let candidates = boundaries
            .windows(2)
            .enumerate()
            .map(|(index, boundary)| {
                let start = boundary[0];
                let end = boundary[1];
                serde_json::json!({
                    "candidate_id": format!("asset-{}-scene-{}-{}", asset.id.0, start.0, end.0),
                    "candidate_index": index,
                    "asset_id": asset.id.0,
                    "source_range": {"start": start.0, "end": end.0},
                    "duration_frames": end.0 - start.0,
                    "boundary_provenance": {
                        "start": if let Some(confidence) = cuts.get(&start) {
                            serde_json::json!({"kind": "scene_cut", "source_frame": start.0, "confidence_basis_points": confidence})
                        } else {
                            serde_json::json!({"kind": "requested_range_start", "source_frame": start.0})
                        },
                        "end": if let Some(confidence) = cuts.get(&end) {
                            serde_json::json!({"kind": "scene_cut", "source_frame": end.0, "confidence_basis_points": confidence})
                        } else {
                            serde_json::json!({"kind": "requested_range_end", "source_frame": end.0})
                        },
                    },
                })
            })
            .collect::<Vec<_>>();
        let minimum_duration_frames = args.minimum_duration_frames.map(|duration| duration.0);
        let eligible_candidates = candidates
            .iter()
            .filter(|candidate| {
                minimum_duration_frames.is_none_or(|minimum| {
                    candidate["duration_frames"]
                        .as_i64()
                        .is_some_and(|duration| duration >= minimum)
                })
            })
            .collect::<Vec<_>>();
        let selected_positions = match candidate_selection {
            ShotBoardCandidateSelection::Page => {
                let offset = args.candidate_offset.unwrap_or(0);
                if offset >= eligible_candidates.len() {
                    return Ok(error_text(format!(
                        "candidate_offset {offset} is outside 0..{} for the {} eligible candidates in this source range",
                        eligible_candidates.len().saturating_sub(1),
                        eligible_candidates.len()
                    )));
                }
                (offset..(offset + usize::from(candidate_count)).min(eligible_candidates.len()))
                    .collect::<Vec<_>>()
            }
            ShotBoardCandidateSelection::Coverage => coverage_candidate_positions(
                eligible_candidates.len(),
                usize::from(candidate_count),
            ),
        };
        let selected = selected_positions
            .iter()
            .map(|&position| eligible_candidates[position].clone())
            .collect::<Vec<_>>();

        let source_range = source_in..source_out;
        let duration = match map_source_range_to_project(source_range.clone(), asset.fps, asset.fps)
        {
            Ok(duration) => duration,
            Err(error) => return Ok(error_text(error.to_string())),
        };
        let temporary = Arc::new(Document {
            tracks: vec![Track {
                id: TrackId(1),
                kind: TrackKind::Video,
                sync_lock: true,
                clips: vec![Clip {
                    id: ClipId(1),
                    asset: asset.id,
                    source_range,
                    content: ClipContent::Media,
                    timeline_start: TimeCode::ZERO,
                    effects: Vec::new(),
                    transition_in: None,
                    link: None,
                    audio_gain_tenth_db: 0,
                    audio_fade_in_frames: TimeCode::ZERO,
                    audio_fade_out_frames: TimeCode::ZERO,
                    speed_percent: 100,
                }],
            }],
            media_pool: vec![asset.clone()],
            fps: asset.fps,
            resolution: asset.resolution.unwrap_or((1_920, 1_080)),
            duration,
            ..Document::default()
        });
        let mut images = Vec::new();
        let mut cells = Vec::new();
        for candidate in &selected {
            let candidate_range = candidate["source_range"]
                .as_object()
                .expect("candidate source range");
            let candidate_start =
                TimeCode(candidate_range["start"].as_i64().expect("candidate start"));
            let candidate_end = TimeCode(candidate_range["end"].as_i64().expect("candidate end"));
            for (evidence_index, source_frame) in
                shot_board_evidence_frames(candidate_start..candidate_end)
                    .into_iter()
                    .enumerate()
            {
                let evidence = ["start", "middle", "end"][evidence_index];
                let local_frame = TimeCode(source_frame.0 - source_in.0);
                match self.analysis.thumbnail_for_document(
                    Arc::clone(&temporary),
                    local_frame,
                    max_width,
                ) {
                    Ok(image) => images.push(image),
                    Err(error) => return Ok(error_text(error.to_string())),
                }
                cells.push(serde_json::json!({
                    "cell": cells.len() + 1,
                    "candidate_id": candidate["candidate_id"].clone(),
                    "candidate_index": candidate["candidate_index"].clone(),
                    "evidence": evidence,
                    "asset_id": asset.id.0,
                    "source_frame": source_frame.0,
                    "source_range": candidate["source_range"].clone(),
                }));
            }
        }
        let sheet = compose_contact_sheet(&images)?;
        let png = encode_png(&sheet)?;
        let manifest = serde_json::json!({
            "timeline_revision": revision.0,
            "asset_id": asset.id.0,
            "source_range": {"start": source_in.0, "end": source_out.0},
            "status": "ready",
            "scene_confidence_threshold_basis_points": minimum_confidence_basis_points,
            "minimum_duration_frames": minimum_duration_frames,
            "candidate_selection": candidate_selection.as_str(),
            "candidate_offset": (candidate_selection == ShotBoardCandidateSelection::Page).then(|| args.candidate_offset.unwrap_or(0)),
            "candidate_count": selected.len(),
            "requested_candidate_count": candidate_count,
            "returned_candidates": selected.len(),
            "filtered_candidates": eligible_candidates.len(),
            "total_candidates": candidates.len(),
            "selected_eligible_candidate_positions": selected_positions,
            "selected_candidate_indexes": selected.iter().map(|candidate| candidate["candidate_index"].clone()).collect::<Vec<_>>(),
            "next_candidate_offset": (candidate_selection == ShotBoardCandidateSelection::Page).then(|| {
                let offset = args.candidate_offset.unwrap_or(0);
                (offset + selected.len() < eligible_candidates.len()).then_some(offset + selected.len())
            }).flatten(),
            "evidence_per_candidate": SHOT_BOARD_EVIDENCE_PER_CANDIDATE,
            "candidates": selected,
            "cells": cells,
            "sheet": {"width": sheet.width, "height": sheet.height},
        });
        let mut result = CallToolResult::success(vec![
            ContentBlock::text(format!(
                "source shot board asset={} ready: returned {} of {} eligible candidates (selection={}, {}), {} evidence cells, sheet={}x{}; candidate ranges are in structured content",
                asset.id,
                selected.len(),
                eligible_candidates.len(),
                candidate_selection.as_str(),
                if candidate_selection == ShotBoardCandidateSelection::Page {
                    format!("offset={}", args.candidate_offset.unwrap_or(0))
                } else {
                    "full-range coverage".to_owned()
                },
                cells.len(),
                sheet.width,
                sheet.height,
            )),
            ContentBlock::image(BASE64.encode(png), "image/png"),
        ]);
        result.structured_content = Some(manifest);
        Ok(result)
    }

    fn source_availability_error(
        &self,
        asset: &MediaAsset,
        consumer: &str,
    ) -> Option<CallToolResult> {
        let availability = self.analysis.media_availability(asset);
        if matches!(
            availability.kind,
            MediaAvailabilityKind::OnlineVerified | MediaAvailabilityKind::OnlineUnverified
        ) {
            return None;
        }
        Some(error_structured(
            format!(
                "{consumer} cannot read asset {} at {}: {:?}",
                asset.id,
                asset.path.display(),
                availability.kind
            ),
            serde_json::json!({
                "asset_id": asset.id.0,
                "path": asset.path,
                "availability": availability,
                "consumer": consumer,
            }),
        ))
    }

    pub(super) fn ensure_verified_patched_sources(
        &self,
        document: &Document,
        operations: &[Operation],
    ) -> Result<(), String> {
        let asset_ids = operations
            .iter()
            .filter_map(|operation| match operation {
                Operation::PatchedThreePointEdit { asset, .. } => Some(*asset),
                _ => None,
            })
            .collect::<BTreeSet<_>>();
        self.ensure_verified_source_assets(document, &asset_ids)
    }

    pub(super) fn ensure_verified_source_assets(
        &self,
        document: &Document,
        asset_ids: &BTreeSet<AssetId>,
    ) -> Result<(), String> {
        for asset_id in asset_ids {
            let Some(asset) = document.asset(*asset_id) else {
                return Err(format!(
                    "patched_three_point_edit references missing asset {asset_id}"
                ));
            };
            let availability = self.analysis.media_availability(asset);
            if availability.kind != MediaAvailabilityKind::OnlineVerified {
                return Err(format!(
                    "patched_three_point_edit requires asset {asset_id} to be online_verified at preparation and commit; current availability is {:?} ({})",
                    availability.kind,
                    availability
                        .reason
                        .as_deref()
                        .unwrap_or("no backend reason supplied")
                ));
            }
        }
        Ok(())
    }

    pub(super) fn document_availability_error(
        &self,
        document: &Document,
        consumer: &str,
    ) -> Option<CallToolResult> {
        // An offline item sitting unused in the media pool must not block a
        // timeline proof. Only source-backed clips can contribute decoded
        // pixels; titles are project-native and need no source file.
        let mut inspected = BTreeSet::new();
        document.tracks.iter().find_map(|track| {
            track.clips.iter().find_map(|clip| {
                if matches!(clip.content, ClipContent::Title(_)) || !inspected.insert(clip.asset) {
                    return None;
                }
                document
                    .asset(clip.asset)
                    .and_then(|asset| self.source_availability_error(asset, consumer))
            })
        })
    }

    #[allow(clippy::too_many_lines)]
    pub(super) fn source_info(&self, args: &SourceInfoArgs) -> Result<CallToolResult, McpError> {
        let (revision, document) = self.snapshot()?;
        let Some(asset) = document.asset(args.asset_id) else {
            return Ok(error_text(format!(
                "asset {} does not exist",
                args.asset_id
            )));
        };
        let source_in = args.source_in.unwrap_or(TimeCode::ZERO);
        let source_out = args.source_out.unwrap_or(asset.duration);
        if source_in < TimeCode::ZERO || source_out > asset.duration || source_out <= source_in {
            return Ok(error_text(format!(
                "source monitor range {source_in}..{source_out} is outside asset {} range 0..{}",
                asset.id, asset.duration
            )));
        }

        let transcript = match self.analysis.transcript_status(asset) {
            TranscriptStatus::Ready(transcript) => Some(transcript),
            _ => None,
        };
        let words = transcript
            .as_ref()
            .map(|transcript| {
                transcript
                    .words
                    .iter()
                    .filter(|word| word.source_end > source_in && word.source_start < source_out)
                    .map(|word| {
                        serde_json::json!({
                            "text": word.text,
                            "speaker": word.speaker,
                            "source_start": word.source_start.0,
                            "source_end": word.source_end.0,
                        })
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let speakers = transcript
            .as_ref()
            .into_iter()
            .flat_map(|transcript| &transcript.words)
            .filter(|word| word.source_end > source_in && word.source_start < source_out)
            .filter_map(|word| word.speaker.as_deref())
            .map(str::to_owned)
            .collect::<BTreeSet<_>>();
        let scenes = match self.analysis.scene_status(asset) {
            SceneStatus::Ready(scenes) => scenes
                .changes
                .iter()
                .filter(|change| {
                    change.source_frame >= source_in && change.source_frame < source_out
                })
                .map(|change| {
                    serde_json::json!({
                        "source_frame": change.source_frame.0,
                        "confidence_basis_points": change.confidence_basis_points,
                    })
                })
                .collect::<Vec<_>>(),
            _ => Vec::new(),
        };
        let beats = match self.analysis.beat_status(asset) {
            BeatStatus::Ready(beats) => beats
                .beats
                .iter()
                .filter(|beat| beat.source_frame >= source_in && beat.source_frame < source_out)
                .map(|beat| {
                    serde_json::json!({
                        "source_frame": beat.source_frame.0,
                        "strength_basis_points": beat.strength_basis_points,
                    })
                })
                .collect::<Vec<_>>(),
            _ => Vec::new(),
        };
        let availability = self.analysis.media_availability(asset);
        let destinations = serde_json::json!({
            "video": document
                .tracks
                .iter()
                .filter(|track| {
                    track.kind == TrackKind::Video && asset.kind.supports(TrackKind::Video)
                })
                .map(|track| {
                    serde_json::json!({
                        "track_id": track.id.0,
                        "kind": track.kind,
                        "sync_lock": track.sync_lock,
                    })
                })
                .collect::<Vec<_>>(),
            "audio": document
                .tracks
                .iter()
                .filter(|track| {
                    track.kind == TrackKind::Audio && asset.kind.supports(TrackKind::Audio)
                })
                .map(|track| {
                    serde_json::json!({
                        "track_id": track.id.0,
                        "kind": track.kind,
                        "sync_lock": track.sync_lock,
                    })
                })
                .collect::<Vec<_>>(),
        });
        let value = serde_json::json!({
            "timeline_revision": revision.0,
            "asset": {
                "id": asset.id.0,
                "name": asset.name,
                "path": asset.path,
                "kind": asset.kind,
                "duration": asset.duration.0,
                "fps": {
                    "numerator": asset.fps.numerator(),
                    "denominator": asset.fps.denominator(),
                },
                "resolution": asset.resolution,
                "color_description": asset.color_description,
                "persisted_fingerprint": asset.source_fingerprint,
                "availability": availability,
            },
            "source_monitor": {
                "source_in": source_in.0,
                "source_out": source_out.0,
                "duration": source_out.0 - source_in.0,
                "in_marked": args.source_in.is_some(),
                "out_marked": args.source_out.is_some(),
            },
            "destinations": destinations,
            "speakers": speakers,
            "words": words,
            "scene_changes": scenes,
            "beats": beats,
            "analysis_jobs": self.analysis.analysis_jobs(asset),
        });
        Ok(success_structured(
            format!(
                "source asset={} range={}..{} words={} scenes={} beats={}\n{}",
                asset.id,
                source_in,
                source_out,
                value["words"].as_array().map_or(0, Vec::len),
                value["scene_changes"].as_array().map_or(0, Vec::len),
                value["beats"].as_array().map_or(0, Vec::len),
                value
            ),
            value,
        ))
    }

    #[allow(clippy::too_many_lines)]
    pub(super) fn search_media(&self, args: &MediaSearchArgs) -> Result<CallToolResult, McpError> {
        let document = self.document()?;
        let query = args
            .query
            .as_deref()
            .map(str::trim)
            .filter(|query| !query.is_empty())
            .map(str::to_lowercase);
        let speaker = args
            .speaker
            .as_deref()
            .map(str::trim)
            .filter(|speaker| !speaker.is_empty())
            .map(str::to_lowercase);
        let limit = args.limit.unwrap_or(25).clamp(1, 100);
        let mut matches = Vec::new();

        for asset in &document.media_pool {
            if args.kind.is_some_and(|kind| kind != asset.kind)
                || args
                    .min_width
                    .is_some_and(|minimum| asset.resolution.is_none_or(|value| value.0 < minimum))
                || args
                    .min_height
                    .is_some_and(|minimum| asset.resolution.is_none_or(|value| value.1 < minimum))
                || args
                    .min_duration_frames
                    .is_some_and(|minimum| asset.duration < minimum)
            {
                continue;
            }

            let transcript = match self.analysis.transcript_status(asset) {
                TranscriptStatus::Ready(transcript) => Some(transcript),
                _ => None,
            };
            if args
                .has_transcript
                .is_some_and(|required| required != transcript.is_some())
            {
                continue;
            }
            let scene_count = match self.analysis.scene_status(asset) {
                SceneStatus::Ready(scenes) => scenes.changes.len(),
                _ => 0,
            };
            if args
                .min_scene_count
                .is_some_and(|minimum| scene_count < minimum)
            {
                continue;
            }
            let beat_count = match self.analysis.beat_status(asset) {
                BeatStatus::Ready(beats) => beats.beats.len(),
                _ => 0,
            };
            if args
                .min_beat_count
                .is_some_and(|minimum| beat_count < minimum)
            {
                continue;
            }

            let speaker_labels = transcript
                .as_ref()
                .into_iter()
                .flat_map(|transcript| &transcript.words)
                .filter_map(|word| word.speaker.as_deref())
                .map(str::to_owned)
                .collect::<BTreeSet<_>>();
            if let Some(speaker) = speaker.as_deref()
                && !speaker_labels
                    .iter()
                    .any(|label| label.to_lowercase() == speaker)
            {
                continue;
            }

            let name_match = query.as_ref().is_some_and(|query| {
                asset.name.to_lowercase().contains(query)
                    || asset.path.to_string_lossy().to_lowercase().contains(query)
            });
            let matching_words = transcript
                .as_ref()
                .into_iter()
                .flat_map(|transcript| &transcript.words)
                .filter(|word| {
                    query.as_ref().is_some_and(|query| {
                        word.text.to_lowercase().contains(query)
                            || word
                                .speaker
                                .as_ref()
                                .is_some_and(|speaker| speaker.to_lowercase().contains(query))
                    })
                })
                .collect::<Vec<_>>();
            if query.is_some() && !name_match && matching_words.is_empty() {
                continue;
            }
            let score = usize::from(name_match) * 100 + matching_words.len().min(99);
            let word_matches = matching_words
                .into_iter()
                .take(12)
                .map(|word| {
                    serde_json::json!({
                        "text": word.text,
                        "speaker": word.speaker,
                        "source_start": word.source_start.0,
                        "source_end": word.source_end.0,
                    })
                })
                .collect::<Vec<_>>();
            matches.push((
                score,
                asset.id,
                serde_json::json!({
                    "asset_id": asset.id.0,
                    "name": asset.name,
                    "path": asset.path,
                    "kind": asset.kind,
                    "duration": asset.duration.0,
                    "fps": {
                        "numerator": asset.fps.numerator(),
                        "denominator": asset.fps.denominator(),
                    },
                    "resolution": asset.resolution,
                    "score": score,
                    "word_matches": word_matches,
                    "speakers": speaker_labels,
                    "scene_count": scene_count,
                    "beat_count": beat_count,
                    "analysis_jobs": self.analysis.analysis_jobs(asset),
                }),
            ));
        }
        matches.sort_by_key(|(score, asset, _)| (std::cmp::Reverse(*score), *asset));
        let total_matches = matches.len();
        let hits = matches
            .into_iter()
            .take(limit)
            .map(|(_, _, hit)| hit)
            .collect::<Vec<_>>();
        let value = serde_json::json!({
            "query": args.query,
            "speaker": args.speaker,
            "total_matches": total_matches,
            "returned": hits.len(),
            "hits": hits,
        });
        Ok(success_structured(
            format!(
                "media search matched {} asset(s), returned {}\n{}",
                total_matches, value["returned"], value
            ),
            value,
        ))
    }
}

/// Three source-monitor views per candidate. The last sample is always the
/// last visible frame, which makes an embedded fade or abrupt pre-cut visible
/// whenever the candidate is long enough to contain one. This is evidence,
/// not a claim that the server detected a fade.
fn shot_board_evidence_frames(range: std::ops::Range<TimeCode>) -> Vec<TimeCode> {
    storyboard_sample_frames(&range, SHOT_BOARD_EVIDENCE_PER_CANDIDATE)
}

/// Return stable positions from an eligible candidate list for a bounded,
/// whole-range inspection. The integer interpolation deliberately avoids
/// floating point rounding so every caller gets identical candidate ids.
pub(super) fn coverage_candidate_positions(
    eligible_count: usize,
    requested_count: usize,
) -> Vec<usize> {
    let count = eligible_count.min(requested_count);
    if count == 0 {
        return Vec::new();
    }
    if count == 1 {
        return vec![0];
    }

    let last = eligible_count.saturating_sub(1);
    let divisor = count.saturating_sub(1);
    (0..count)
        .map(|index| index.saturating_mul(last) / divisor)
        .collect()
}
