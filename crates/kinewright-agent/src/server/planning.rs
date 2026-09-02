//! Edit planners: dialogue assembly, beat pacing/montage, music fit, multicam, and loudness.

use super::*;

impl KinewrightMcp {
    /// Deterministic completion feedback: the plan result itself reports how
    /// much cuttable silence remains, so an agent asked to remove dead air
    /// cannot mistake a partial plan for a finished one.
    pub(super) fn remaining_silence_footer(&self, document: &kinewright_core::Document) -> String {
        let Ok(spans) = self.analysis.timeline_silences(
            document,
            None,
            TimeCode(DEFAULT_MINIMUM_SILENCE_FRAMES),
        ) else {
            return String::new();
        };
        let mut cuttable = 0_usize;
        for span in &spans {
            let Some(asset) = document.asset(span.asset) else {
                continue;
            };
            let words = match self.analysis.transcript_status(asset) {
                TranscriptStatus::Ready(transcript) => Some(transcript),
                _ => None,
            };
            cuttable += crate::silence::shrink_silence_span_for_cutting_with_transcript(
                kinewright_core::SilenceSpan {
                    source_start: span.source_start,
                    source_end: span.source_end,
                },
                asset.fps,
                words.as_ref().map(|transcript| transcript.words.as_slice()),
            )
            .len();
        }
        let pending = document
            .media_pool
            .iter()
            .filter(|asset| {
                !matches!(
                    self.analysis.silence_status(asset),
                    SilenceStatus::Ready(_) | SilenceStatus::NoAudio
                )
            })
            .count();
        let mut footer = if cuttable == 0 {
            "\nno cuttable silence remains on the timeline".to_owned()
        } else {
            format!("\ncuttable silence spans remaining on the timeline: {cuttable}")
        };
        if pending > 0 {
            let _ = write!(footer, " (silence analysis pending for {pending} asset(s))");
        }
        footer
    }

    pub(super) fn plan_dialogue_assembly(
        &self,
        args: &DialogueAssemblyPlanArgs,
    ) -> Result<CallToolResult, McpError> {
        let (revision, document) = self.snapshot()?;
        if let Err(error) = validate_dialogue_assembly_assets(args) {
            return Ok(error_text(error));
        }
        let target_track = args.target_track_id;
        if document.tracks.iter().all(|track| track.id != target_track) {
            return Ok(error_text(format!(
                "target track {target_track} does not exist"
            )));
        }
        let minimum = args.minimum_silence_source_frames.unwrap_or(TimeCode(20));
        if minimum <= TimeCode::ZERO {
            return Ok(error_text("minimum_silence_source_frames must be positive"));
        }
        let remove_fillers = args.remove_fillers.unwrap_or(true);
        let pacing = match dialogue_pacing_settings(args) {
            Ok(settings) => settings,
            Err(error) => return Ok(error_text(error)),
        };
        let mut at = args.timeline_start.unwrap_or(TimeCode::ZERO);
        if at < TimeCode::ZERO {
            return Ok(error_text("timeline_start must be non-negative"));
        }
        let mut operations = Vec::new();
        let mut selections = Vec::new();
        for (index, asset_id) in args.asset_ids.iter().enumerate() {
            let Some(asset) = document.asset(*asset_id).cloned() else {
                return Ok(error_text(format!("asset {asset_id} does not exist")));
            };
            let source_range = match dialogue_source_range(args, index, &asset) {
                Ok(range) => range,
                Err(error) => return Ok(error_text(error)),
            };
            let (transcript, silences) = match self.ready_dialogue_analysis(&asset, minimum) {
                Ok(analysis) => analysis,
                Err(result) => return Ok(result),
            };
            let ranges = dialogue_keep_ranges(
                &asset,
                &transcript,
                &silences,
                minimum,
                remove_fillers,
                pacing,
                source_range,
            );
            if ranges.is_empty() {
                return Ok(error_text(format!(
                    "asset {asset_id} has no source frames left after dialogue cleanup"
                )));
            }
            for source in &ranges {
                operations.push(Operation::AddClip {
                    track: target_track,
                    asset: *asset_id,
                    at,
                    source: source.clone(),
                });
                let duration = map_source_range_to_project(source.clone(), asset.fps, document.fps)
                    .map_err(|error| McpError::internal_error(error.to_string(), None))?;
                at = at.checked_add(duration).ok_or_else(|| {
                    McpError::internal_error("dialogue assembly overflowed", None)
                })?;
            }
            let selection = dialogue_selection(&ranges, &transcript, &silences, pacing, minimum);
            selections.push(selection);
        }

        let plan = match self.prepare_operations(revision, &document, operations) {
            Ok(plan) => plan,
            Err(error) => {
                return Ok(error_text(format!(
                    "dialogue assembly does not fit the current target track: {error}"
                )));
            }
        };
        let structured = serde_json::json!({
            "timeline_revision": revision,
            "retained_pause_source_frames": pacing.retained_pause,
            "filler_padding_source_frames": pacing.filler_padding,
            "maximum_filler_bridge_pause_source_frames": pacing.maximum_filler_bridge_pause,
            "selections": selections,
            "resulting_range": {
                "start": args.timeline_start.unwrap_or(TimeCode::ZERO),
                "end": at,
            },
            "prepared_edit_plan": {
                "plan_id": plan.id,
                "expected_revision": revision,
                "preview": plan.preview,
            },
        });
        Ok(success_structured(
            format!(
                "prepared {} gapless dialogue clip(s) from {} ordered asset(s) as edit plan {}; inspect the preview, then commit it at timeline revision {revision}",
                plan.preview.operation_count,
                args.asset_ids.len(),
                plan.id,
            ),
            structured,
        ))
    }

    fn ready_dialogue_analysis(
        &self,
        asset: &MediaAsset,
        minimum: TimeCode,
    ) -> Result<(Arc<AssetTranscript>, Arc<AssetSilences>), CallToolResult> {
        let transcript = match self.analysis.transcript_status(asset) {
            TranscriptStatus::Ready(transcript) => transcript,
            status => {
                if status == TranscriptStatus::NotRequested {
                    self.analysis.request_transcription(asset.clone());
                }
                return Err(error_text(format!(
                    "asset {} transcript is not ready: {}",
                    asset.id,
                    render_asset_transcript(asset.id, &status)
                )));
            }
        };
        let silences = match self.analysis.silence_status(asset) {
            SilenceStatus::Ready(silences) => silences,
            status => {
                if status == SilenceStatus::NotRequested {
                    self.analysis.request_silence_detection(asset.clone());
                }
                return Err(error_text(format!(
                    "asset {} silence analysis is not ready: {}",
                    asset.id,
                    render_asset_silences(asset.id, &status, minimum, Some(transcript.as_ref()),)
                )));
            }
        };
        Ok((transcript, silences))
    }

    pub(super) fn plan_beat_pacing(
        &self,
        args: BeatPacingPlanArgs,
    ) -> Result<CallToolResult, McpError> {
        let (revision, document) = self.snapshot()?;
        let minimum_strength = match args.min_strength {
            Some(value) => match percentage_to_basis_points(value, "min_strength") {
                Ok(value) => value,
                Err(error) => return Ok(error_text(error)),
            },
            None => DEFAULT_BEAT_STRENGTH_BASIS_POINTS,
        };
        let range = args.range.map(|range| range.start..range.end);
        let referenced_assets = document
            .tracks
            .iter()
            .flat_map(|track| &track.clips)
            .filter(|clip| clip.content.is_media())
            .map(|clip| clip.asset)
            .collect::<BTreeSet<_>>();
        let mut pending = Vec::new();
        let mut unavailable = Vec::new();
        for asset_id in &referenced_assets {
            let Some(asset) = document.asset(*asset_id) else {
                continue;
            };
            if self.analysis.beat_status(asset) == BeatStatus::NotRequested {
                self.analysis.request_beat_detection(asset.clone());
            }
            match self.analysis.beat_status(asset) {
                BeatStatus::Ready(_) | BeatStatus::NoAudio => {}
                BeatStatus::Failed(reason) => {
                    unavailable.push((*asset_id, format!("failed: {reason}")));
                }
                BeatStatus::Cancelled => {
                    unavailable.push((*asset_id, "cancelled".to_owned()));
                }
                BeatStatus::NotRequested
                | BeatStatus::Queued
                | BeatStatus::Hashing
                | BeatStatus::Analyzing { .. } => pending.push(*asset_id),
            }
        }
        let analysis_state = if !unavailable.is_empty() {
            let reason = unavailable
                .iter()
                .map(|(asset, reason)| format!("asset {asset}: {reason}"))
                .collect::<Vec<_>>()
                .join("; ");
            TimelineBeatAnalysisState::Unavailable {
                asset_ids: unavailable.into_iter().map(|(asset, _)| asset).collect(),
                reason,
            }
        } else if pending.is_empty() {
            TimelineBeatAnalysisState::Ready
        } else {
            TimelineBeatAnalysisState::Pending { asset_ids: pending }
        };
        let beats = match self
            .analysis
            .timeline_beats(&document, range.clone(), minimum_strength)
        {
            Ok(beats) => beats,
            Err(error) => return Ok(error_text(error.to_string())),
        };
        let plan = match beat_pacing_plan(
            &document,
            args.clip_id,
            range,
            &beats,
            &analysis_state,
            minimum_strength,
            args.minimum_spacing_frames.unwrap_or(TimeCode(6)),
        ) {
            Ok(plan) => plan,
            Err(error) => return Ok(error_text(error.to_string())),
        };
        let prepared = match self.prepare_operations(revision, &document, plan.operations.clone()) {
            Ok(prepared) => prepared,
            Err(error) => {
                return Ok(error_text(format!(
                    "beat pacing plan does not fit the current timeline: {error}"
                )));
            }
        };
        let structured = serde_json::json!({
            "timeline_revision": revision.0,
            "plan": plan,
            "prepared_edit_plan": {
                "plan_id": prepared.id,
                "expected_revision": revision,
                "preview": prepared.preview,
            },
        });
        Ok(success_structured(
            format!(
                "prepared {} beat-aligned split(s) for clip {} as edit plan {}; inspect the selected onsets and preview, then commit it at timeline revision {revision}",
                plan.operations.len(),
                plan.target_clip,
                prepared.id,
            ),
            structured,
        ))
    }

    #[allow(clippy::too_many_lines)]
    pub(super) fn plan_beat_montage(
        &self,
        args: &BeatMontagePlanArgs,
    ) -> Result<CallToolResult, McpError> {
        let (revision, document) = self.snapshot()?;
        let Some(music_asset) = document.asset(args.music_asset_id) else {
            return Ok(error_text(format!(
                "music asset {} does not exist",
                args.music_asset_id
            )));
        };
        if !music_asset.kind.supports(TrackKind::Audio) {
            return Ok(error_text(format!(
                "music asset {} does not contain audio",
                args.music_asset_id
            )));
        }
        let minimum_strength = match args.min_strength {
            Some(value) => match percentage_to_basis_points(value, "min_strength") {
                Ok(value) => value,
                Err(error) => return Ok(error_text(error)),
            },
            None => DEFAULT_BEAT_STRENGTH_BASIS_POINTS,
        };
        let range = args.timeline_range.start..args.timeline_range.end;
        let mut status = self.analysis.beat_status(music_asset);
        if status == BeatStatus::NotRequested {
            self.analysis.request_beat_detection(music_asset.clone());
            status = self.analysis.beat_status(music_asset);
        }
        let analysis_state = beat_montage_analysis_state(args.music_asset_id, &status);
        let beats =
            match self
                .analysis
                .timeline_beats(&document, Some(range.clone()), minimum_strength)
            {
                Ok(beats) => beats,
                Err(error) => return Ok(error_text(error.to_string())),
            };
        let selects = args
            .selects
            .iter()
            .map(|select| BeatMontageSelect {
                asset: select.asset_id,
                source_range: select.source_range.start..select.source_range.end,
            })
            .collect::<Vec<_>>();
        let minimum_shot_frames = args.minimum_shot_frames.unwrap_or(TimeCode(20));
        let maximum_shot_frames = args.maximum_shot_frames.unwrap_or(TimeCode(120));
        let (plan, anchor_repair) = match (
            args.cut_anchor_frames.as_deref(),
            args.anchor_repair.as_ref(),
        ) {
            (Some(preferred_anchors), Some(settings)) => {
                if settings.maximum_movement_frames < TimeCode::ZERO {
                    return Ok(error_text(
                        "anchor_repair.maximum_movement_frames must be non-negative; repair is always bounded and never silently broadened",
                    ));
                }
                if settings
                    .locked_anchor_indices
                    .windows(2)
                    .any(|pair| pair[0] >= pair[1])
                {
                    return Ok(error_text(
                        "anchor_repair.locked_anchor_indices must be strictly increasing and unique",
                    ));
                }
                let (plan, report) = match beat_montage_plan_near_anchors_with_report(
                    &document,
                    args.target_track_id,
                    args.music_asset_id,
                    range.clone(),
                    &selects,
                    preferred_anchors,
                    &beats,
                    &analysis_state,
                    minimum_strength,
                    minimum_shot_frames,
                    maximum_shot_frames,
                    args.mode,
                    Some(settings.maximum_movement_frames),
                    &settings.locked_anchor_indices,
                    args.cadence,
                ) {
                    Ok(result) => result,
                    Err(error) => {
                        let failure = error.to_string();
                        let recovery = beat_montage_plan_near_anchors_with_report(
                            &document,
                            args.target_track_id,
                            args.music_asset_id,
                            range,
                            &selects,
                            preferred_anchors,
                            &beats,
                            &analysis_state,
                            minimum_strength,
                            minimum_shot_frames,
                            maximum_shot_frames,
                            args.mode,
                            None,
                            &[],
                            args.cadence,
                        )
                        .ok()
                        .map(|(suggested_plan, suggested_report)| {
                            let shot_durations = suggested_plan
                                .shots
                                .iter()
                                .map(|shot| {
                                    shot.timeline_range
                                        .end
                                        .0
                                        .saturating_sub(shot.timeline_range.start.0)
                                })
                                .collect::<Vec<_>>();
                            serde_json::json!({
                                "cut_anchor_frames": suggested_report.resolved_anchors,
                                "shot_durations": shot_durations,
                                "signed_delta_frames": suggested_report.signed_deltas,
                                "maximum_absolute_delta_frames": suggested_report.maximum_absolute_delta,
                                "total_absolute_delta_frames": suggested_report.total_absolute_delta,
                                "exact_retry_patch": {
                                    "cut_anchor_frames": suggested_report.resolved_anchors,
                                    "anchor_repair": {
                                        "maximum_movement_frames": 0,
                                        "locked_anchor_indices": [],
                                    },
                                },
                            })
                        });
                        let message = format!(
                            "beat montage anchor repair could not satisfy preferred anchors within maximum_movement_frames={}: {failure}; revise preferred anchors, increase the explicit bound, unlock an anchor, or adjust source envelopes and retry{}",
                            settings.maximum_movement_frames,
                            if recovery.is_some() {
                                "; the structured error includes the nearest globally feasible source- and cadence-valid anchor schedule plus an exact_retry_patch, so reuse it instead of guessing"
                            } else {
                                ""
                            }
                        );
                        if let Some(recovery) = recovery {
                            return Ok(error_structured(
                                message,
                                serde_json::json!({
                                    "status": "bounded_anchor_repair_infeasible",
                                    "error": failure,
                                    "requested_maximum_movement_frames": settings.maximum_movement_frames,
                                    "requested_locked_anchor_indices": settings.locked_anchor_indices,
                                    "nearest_globally_feasible": recovery,
                                }),
                            ));
                        }
                        return Ok(error_text(message));
                    }
                };
                let repaired = report.signed_deltas.iter().any(|delta| *delta != 0);
                let evidence = serde_json::json!({
                    "repaired": repaired,
                    "preferred_anchor_frames": report.preferred_anchors,
                    "resolved_anchor_frames": report.resolved_anchors,
                    "signed_delta_frames": report.signed_deltas,
                    "absolute_delta_frames": report.absolute_deltas,
                    "maximum_absolute_delta_frames": report.maximum_absolute_delta,
                    "total_absolute_delta_frames": report.total_absolute_delta,
                    "maximum_movement_frames": settings.maximum_movement_frames,
                    "locked_anchor_indices": settings.locked_anchor_indices,
                });
                (plan, Some(evidence))
            }
            (Some(cut_anchor_frames), None) => {
                match beat_montage_plan_with_anchors(
                    &document,
                    args.target_track_id,
                    args.music_asset_id,
                    range,
                    &selects,
                    cut_anchor_frames,
                    &beats,
                    &analysis_state,
                    minimum_strength,
                    minimum_shot_frames,
                    maximum_shot_frames,
                    args.mode,
                ) {
                    Ok(plan) => (plan, None),
                    Err(error) => return Ok(error_text(error.to_string())),
                }
            }
            (None, Some(_)) => {
                return Ok(error_text(
                    "anchor_repair requires explicit cut_anchor_frames; supply exactly one fewer preferred anchor than selects or omit anchor_repair",
                ));
            }
            (None, None) => match beat_montage_plan(
                &document,
                args.target_track_id,
                args.music_asset_id,
                range,
                &selects,
                &beats,
                &analysis_state,
                minimum_strength,
                minimum_shot_frames,
                maximum_shot_frames,
                args.mode,
            ) {
                Ok(plan) => (plan, None),
                Err(error) => return Ok(error_text(error.to_string())),
            },
        };
        let cadence_summary = match args.cadence {
            Some(contract) => match validate_beat_montage_plan_cadence(&plan, contract) {
                Ok(summary) => Some(summary),
                Err(error) => {
                    return Ok(error_text(format!(
                        "beat montage cadence contract rejected prepared plan: {error}; revise shot durations or cut anchors and retry"
                    )));
                }
            },
            None => None,
        };
        let prepared = match self.prepare_operations(revision, &document, plan.operations.clone()) {
            Ok(prepared) => prepared,
            Err(error) => {
                return Ok(error_text(format!(
                    "beat montage plan does not fit the current timeline: {error}"
                )));
            }
        };
        let structured = serde_json::json!({
            "timeline_revision": revision.0,
            "plan": plan,
            "cadence": cadence_summary,
            "anchor_repair": anchor_repair,
            "prepared_edit_plan": {
                "plan_id": prepared.id,
                "expected_revision": revision,
                "preview": prepared.preview,
            },
        });
        Ok(success_structured(
            format!(
                "prepared {} model-ordered, source-feasible hard-cut montage shot(s) against music asset {} as edit plan {}; inspect the resolved beat anchors, optional anchor_repair evidence, and preview before committing it at timeline revision {revision}; no transition or retime was added",
                plan.shots.len(),
                plan.music_asset,
                prepared.id,
            ),
            structured,
        ))
    }

    pub(super) fn plan_music_fit(
        &self,
        args: &MusicFitPlanArgs,
    ) -> Result<CallToolResult, McpError> {
        let (revision, document) = self.snapshot()?;
        let Some(asset) = document.asset(args.asset_id) else {
            return Ok(error_text(format!(
                "asset {} does not exist",
                args.asset_id
            )));
        };
        let minimum_strength = match args.min_strength {
            Some(value) => match percentage_to_basis_points(value, "min_strength") {
                Ok(value) => value,
                Err(error) => return Ok(error_text(error)),
            },
            None => DEFAULT_BEAT_STRENGTH_BASIS_POINTS,
        };
        if self.analysis.beat_status(asset) == BeatStatus::NotRequested {
            self.analysis.request_beat_detection(asset.clone());
        }
        let status = self.analysis.beat_status(asset);
        let end_anchor = match (args.preferred_source_end, args.maximum_end_drift_frames) {
            (None, None) => None,
            (Some(preferred_source_end), Some(maximum_drift_frames)) => {
                Some(kinewright_core::MusicEndAnchor {
                    preferred_source_end,
                    maximum_drift_frames,
                })
            }
            (Some(_), None) => {
                return Ok(error_text(
                    "preferred_source_end requires maximum_end_drift_frames; end targeting is always explicitly bounded",
                ));
            }
            (None, Some(_)) => {
                return Ok(error_text(
                    "maximum_end_drift_frames requires preferred_source_end; end targeting is never inferred",
                ));
            }
        };
        let plan = match music_fit_plan_with_end_anchor(
            &document,
            args.track_id,
            args.asset_id,
            args.timeline_range.start..args.timeline_range.end,
            args.preferred_source_start,
            end_anchor,
            &status,
            minimum_strength,
            args.mode,
        ) {
            Ok(plan) => plan,
            Err(error) => return Ok(error_text(error.to_string())),
        };
        let prepared = match self.prepare_operations(revision, &document, plan.operations.clone()) {
            Ok(prepared) => prepared,
            Err(error) => {
                return Ok(error_text(format!(
                    "music fit plan does not fit the current timeline: {error}"
                )));
            }
        };
        let structured = serde_json::json!({
            "timeline_revision": revision.0,
            "plan": plan,
            "prepared_edit_plan": {
                "plan_id": prepared.id,
                "expected_revision": revision,
                "preview": prepared.preview,
            },
        });
        Ok(success_structured(
            format!(
                "prepared {:?} real-time music edit from source frames {}..{} into project frames {}..{} as edit plan {}; inspect the endpoint evidence and preview, then commit it at timeline revision {revision}; no looping or hidden time stretch was used",
                plan.strategy,
                plan.source_range.start.0,
                plan.source_range.end.0,
                plan.timeline_range.start.0,
                plan.timeline_range.end.0,
                prepared.id,
            ),
            structured,
        ))
    }

    pub(super) fn plan_speaker_multicam(
        &self,
        args: SpeakerMulticamPlanArgs,
    ) -> Result<CallToolResult, McpError> {
        let (revision, document) = self.snapshot()?;
        let Some(reference_asset) = document.asset(args.reference_asset_id) else {
            return Ok(error_text(format!(
                "asset {} does not exist",
                args.reference_asset_id
            )));
        };
        let mut transcript_status = self.analysis.transcript_status(reference_asset);
        if transcript_status == TranscriptStatus::NotRequested {
            self.analysis.request_transcription(reference_asset.clone());
            transcript_status = self.analysis.transcript_status(reference_asset);
        }
        let TranscriptStatus::Ready(transcript) = transcript_status else {
            return Ok(error_text(format!(
                "speaker-aware multicam requires a ready diarized transcript for asset {}; current analysis state: {}",
                args.reference_asset_id,
                render_asset_transcript(args.reference_asset_id, &transcript_status),
            )));
        };
        let settings = SpeakerMulticamSettings {
            sync_group: args.sync_group_id,
            target_track: args.target_track_id,
            group_start: args.group_range.start,
            group_end: args.group_range.end,
            record_start: args.record_start,
            maximum_word_gap_frames: args.maximum_word_gap_frames.unwrap_or(TimeCode(3)),
            minimum_shot_frames: args.minimum_shot_frames.unwrap_or(TimeCode(5)),
            assignments: args.assignments,
        };
        let plan = match plan_speaker_multicam(&document, &transcript, &settings) {
            Ok(plan) => plan,
            Err(error) => return Ok(error_text(error.to_string())),
        };
        let prepared = match self.prepare_operations(revision, &document, plan.operations.clone()) {
            Ok(prepared) => prepared,
            Err(error) => {
                return Ok(error_text(format!(
                    "speaker multicam plan does not fit the current timeline: {error}"
                )));
            }
        };
        let structured = serde_json::json!({
            "timeline_revision": revision.0,
            "plan": plan,
            "prepared_edit_plan": {
                "plan_id": prepared.id,
                "expected_revision": revision,
                "preview": prepared.preview,
            },
        });
        Ok(success_structured(
            format!(
                "prepared {} speaker-aware multicam shot(s) from transcript asset {} as edit plan {}; inspect the preview, then commit it at timeline revision {revision}",
                plan.cuts.len(),
                plan.reference_asset,
                prepared.id,
            ),
            structured,
        ))
    }

    pub(super) fn plan_audio_normalization(
        &self,
        args: &AudioNormalizationPlanArgs,
    ) -> Result<CallToolResult, McpError> {
        let (revision, document) = self.snapshot()?;
        let context = match normalization_context(&document, args) {
            Ok(context) => context,
            Err(error) => return Ok(error_text(error)),
        };
        let current = match self.analysis.timeline_loudness(&document) {
            Ok(measurement) => measurement,
            Err(error) => {
                return Ok(error_text(format!(
                    "could not measure timeline audio: {error}"
                )));
            }
        };
        let (operation, predicted) = match verified_normalization_operation(
            self.analysis.as_ref(),
            &document,
            args,
            &context,
            current,
        ) {
            Ok(result) => result,
            Err(error) => return Ok(error_text(error)),
        };
        let prepared = match self.prepare_operations(revision, &document, vec![operation]) {
            Ok(prepared) => prepared,
            Err(error) => {
                return Ok(error_text(format!(
                    "normalization plan does not fit the current timeline: {error}"
                )));
            }
        };
        let current_lufs = current.integrated_lufs_hundredths.unwrap_or_default();
        let predicted_lufs = predicted.integrated_lufs_hundredths.unwrap_or_default();
        Ok(success_structured(
            format!(
                "prepared measured audio normalization from {current_lufs} to {predicted_lufs} LUFS hundredths as edit plan {}; inspect the bus processing and preview, then commit it at timeline revision {revision}",
                prepared.id
            ),
            serde_json::json!({
                "timeline_revision": revision.0,
                "target_lufs_hundredths": args.target_lufs_hundredths,
                "maximum_sample_peak_dbfs_hundredths": args.maximum_sample_peak_dbfs_hundredths,
                "lossy_codec_peak_headroom_hundredths": LOSSY_CODEC_PEAK_HEADROOM_HUNDREDTHS,
                "processing_ceiling_dbfs_hundredths": args.maximum_sample_peak_dbfs_hundredths
                    .saturating_sub(LOSSY_CODEC_PEAK_HEADROOM_HUNDREDTHS),
                "tolerance_hundredths": args.tolerance_hundredths,
                "current": current,
                "predicted": predicted,
                "prepared_edit_plan": {
                    "plan_id": prepared.id,
                    "expected_revision": revision,
                    "preview": prepared.preview,
                },
            }),
        ))
    }
}

pub(super) fn beat_montage_analysis_state(
    music_asset: AssetId,
    status: &BeatStatus,
) -> TimelineBeatAnalysisState {
    match status {
        BeatStatus::Ready(_) => TimelineBeatAnalysisState::Ready,
        BeatStatus::NoAudio => TimelineBeatAnalysisState::Unavailable {
            asset_ids: vec![music_asset],
            reason: "music asset has no audio stream".to_owned(),
        },
        BeatStatus::Cancelled => TimelineBeatAnalysisState::Unavailable {
            asset_ids: vec![music_asset],
            reason: "beat analysis was cancelled".to_owned(),
        },
        BeatStatus::Failed(reason) => TimelineBeatAnalysisState::Unavailable {
            asset_ids: vec![music_asset],
            reason: format!("beat analysis failed: {reason}"),
        },
        BeatStatus::NotRequested
        | BeatStatus::Queued
        | BeatStatus::Hashing
        | BeatStatus::Analyzing { .. } => TimelineBeatAnalysisState::Pending {
            asset_ids: vec![music_asset],
        },
    }
}

struct NormalizationContext {
    tracks: Vec<TrackId>,
    bus_id: AudioBusId,
    first_effect_id: u64,
}

fn normalization_context(
    document: &Document,
    args: &AudioNormalizationPlanArgs,
) -> Result<NormalizationContext, String> {
    if args.track_ids.is_empty() {
        return Err("track_ids must contain at least one audio source track".to_owned());
    }
    let tracks = args.track_ids.iter().copied().collect::<BTreeSet<_>>();
    if tracks.len() != args.track_ids.len() {
        return Err("track_ids must not contain duplicates".to_owned());
    }
    for track in &tracks {
        let candidate = document
            .tracks
            .iter()
            .find(|candidate| candidate.id == *track)
            .ok_or_else(|| format!("track {track} does not exist"))?;
        if candidate.clips.is_empty() {
            return Err(format!("track {track} contains no audio source clips"));
        }
    }
    if let Some(bus) = document
        .audio_mix
        .buses
        .iter()
        .find(|bus| bus.tracks.iter().any(|track| tracks.contains(track)))
    {
        return Err(format!(
            "track selection already intersects audio bus {} ({}); remove or deliberately revise that mix before normalizing",
            bus.id, bus.name
        ));
    }
    if !(-2_400..=-900).contains(&args.target_lufs_hundredths) {
        return Err("target_lufs_hundredths must be in -2400..=-900".to_owned());
    }
    if !(-300..=0).contains(&args.maximum_sample_peak_dbfs_hundredths) {
        return Err("maximum_sample_peak_dbfs_hundredths must be in -300..=0".to_owned());
    }
    if !(25..=300).contains(&args.tolerance_hundredths) {
        return Err("tolerance_hundredths must be in 25..=300".to_owned());
    }
    let bus_id = AudioBusId(
        document
            .audio_mix
            .buses
            .iter()
            .map(|bus| bus.id.0)
            .max()
            .unwrap_or(0)
            .saturating_add(1),
    );
    let first_effect_id = document
        .tracks
        .iter()
        .flat_map(|track| &track.clips)
        .flat_map(|clip| &clip.effects)
        .chain(document.audio_mix.buses.iter().flat_map(|bus| &bus.effects))
        .map(|effect| effect.id.0)
        .max()
        .unwrap_or(0)
        .saturating_add(1);
    Ok(NormalizationContext {
        tracks: tracks.into_iter().collect(),
        bus_id,
        first_effect_id,
    })
}

fn verified_normalization_operation(
    analysis: &dyn Analysis,
    document: &Document,
    args: &AudioNormalizationPlanArgs,
    context: &NormalizationContext,
    current: AudioLoudness,
) -> Result<(Operation, AudioLoudness), String> {
    let current_lufs = current.integrated_lufs_hundredths.ok_or_else(|| {
        "timeline audio is silent; normalization cannot infer a programme level".to_owned()
    })?;
    let current_peak = current
        .sample_peak_dbfs_hundredths
        .ok_or_else(|| "timeline audio has no measurable sample peak".to_owned())?;
    let mut requested_gain = args.target_lufs_hundredths.saturating_sub(current_lufs);
    let mut final_operation = None;
    let mut predicted = current;
    for _ in 0..4 {
        let processing_ceiling = args
            .maximum_sample_peak_dbfs_hundredths
            .saturating_sub(LOSSY_CODEC_PEAK_HEADROOM_HUNDREDTHS);
        let bus = normalization_bus(
            context.bus_id,
            context.first_effect_id,
            context.tracks.clone(),
            requested_gain,
            current_peak,
            processing_ceiling,
        )?;
        let operation = Operation::UpsertAudioBus { bus };
        let mut candidate = document.clone();
        apply_batch(&mut candidate, std::slice::from_ref(&operation))
            .map_err(|error| format!("normalization processing is not applicable: {error}"))?;
        predicted = analysis
            .timeline_loudness(&candidate)
            .map_err(|error| format!("could not verify normalized timeline audio: {error}"))?;
        let predicted_lufs = predicted
            .integrated_lufs_hundredths
            .ok_or_else(|| "normalization unexpectedly produced silent output".to_owned())?;
        final_operation = Some(operation);
        let correction = args.target_lufs_hundredths.saturating_sub(predicted_lufs);
        if correction.unsigned_abs() <= u32::from(args.tolerance_hundredths) {
            break;
        }
        requested_gain = requested_gain.saturating_add(correction);
    }
    let predicted_lufs = predicted
        .integrated_lufs_hundredths
        .ok_or_else(|| "normalized loudness measurement disappeared".to_owned())?;
    let predicted_peak = predicted
        .sample_peak_dbfs_hundredths
        .ok_or_else(|| "normalized peak measurement disappeared".to_owned())?;
    if predicted_lufs.abs_diff(args.target_lufs_hundredths) > u32::from(args.tolerance_hundredths)
        || predicted_peak > args.maximum_sample_peak_dbfs_hundredths
    {
        return Err(format!(
            "normalization could not satisfy the delivery contract: predicted_lufs_hundredths={predicted_lufs}, predicted_peak_dbfs_hundredths={predicted_peak}"
        ));
    }
    Ok((
        final_operation.expect("normalization produced an operation"),
        predicted,
    ))
}

fn round_hundredths_to_tenths(value: i32) -> i64 {
    i64::from(if value >= 0 {
        value.saturating_add(5) / 10
    } else {
        value.saturating_sub(5) / 10
    })
}

fn static_audio_effect(id: EffectId, name: &str, parameters: &[(&str, i64)]) -> Effect {
    Effect {
        id,
        name: name.to_owned(),
        parameters: parameters
            .iter()
            .map(|(name, value)| ((*name).to_owned(), ParamValue::Integer(*value)))
            .collect(),
        keyframes: BTreeMap::new(),
    }
}

fn normalization_bus(
    bus_id: AudioBusId,
    first_effect_id: u64,
    tracks: Vec<TrackId>,
    gain_hundredths: i32,
    measured_peak_hundredths: i32,
    ceiling_hundredths: i32,
) -> Result<AudioBus, String> {
    if !(-6_000..=3_600).contains(&gain_hundredths) {
        return Err(format!(
            "required normalization gain {gain_hundredths} hundredths dB exceeds the supported -6000..=3600 range"
        ));
    }
    let mut effects = Vec::new();
    let mut next_effect_id = first_effect_id;
    if gain_hundredths >= 0 {
        let makeup_hundredths = gain_hundredths.min(2_400);
        let post_gain_hundredths = gain_hundredths.saturating_sub(makeup_hundredths);
        let compression_required =
            measured_peak_hundredths.saturating_add(gain_hundredths) > ceiling_hundredths;
        let (threshold_tenth_db, ratio_hundredths) = if compression_required {
            let numerator = i64::from(ceiling_hundredths)
                .saturating_sub(i64::from(gain_hundredths))
                .saturating_sub(i64::from(measured_peak_hundredths).div_euclid(4));
            let threshold_hundredths = numerator.saturating_mul(4).div_euclid(3).clamp(-6_000, 0);
            (threshold_hundredths.div_euclid(10), 400)
        } else {
            (0, 100)
        };
        effects.push(static_audio_effect(
            EffectId(next_effect_id),
            "audio_compressor",
            &[
                ("threshold_tenth_db", threshold_tenth_db),
                ("ratio_hundredths", ratio_hundredths),
                ("attack_milliseconds", 5),
                ("release_milliseconds", 200),
                (
                    "makeup_gain_tenth_db",
                    round_hundredths_to_tenths(makeup_hundredths),
                ),
            ],
        ));
        next_effect_id = next_effect_id.saturating_add(1);
        if post_gain_hundredths > 0 {
            effects.push(static_audio_effect(
                EffectId(next_effect_id),
                "audio_gain",
                &[(
                    "gain_tenth_db",
                    round_hundredths_to_tenths(post_gain_hundredths),
                )],
            ));
            next_effect_id = next_effect_id.saturating_add(1);
        }
    } else {
        effects.push(static_audio_effect(
            EffectId(next_effect_id),
            "audio_gain",
            &[("gain_tenth_db", round_hundredths_to_tenths(gain_hundredths))],
        ));
        next_effect_id = next_effect_id.saturating_add(1);
    }
    effects.push(static_audio_effect(
        EffectId(next_effect_id),
        "audio_limiter",
        &[(
            "ceiling_tenth_db",
            i64::from(ceiling_hundredths).div_euclid(10),
        )],
    ));
    Ok(AudioBus {
        id: bus_id,
        name: "Delivery normalization".to_owned(),
        tracks,
        effects,
        ducking_sidechain_tracks: Vec::new(),
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct DialoguePacingSettings {
    pub(super) retained_pause: TimeCode,
    pub(super) filler_padding: TimeCode,
    pub(super) maximum_filler_bridge_pause: Option<TimeCode>,
}

fn validate_dialogue_assembly_assets(args: &DialogueAssemblyPlanArgs) -> Result<(), &'static str> {
    if args.asset_ids.is_empty() {
        return Err("dialogue assembly requires at least one ordered asset id");
    }
    if args
        .source_ranges
        .as_ref()
        .is_some_and(|ranges| ranges.len() != args.asset_ids.len())
    {
        return Err("dialogue assembly source_ranges must match asset_ids length");
    }
    Ok(())
}

fn dialogue_source_range(
    args: &DialogueAssemblyPlanArgs,
    index: usize,
    asset: &MediaAsset,
) -> Result<std::ops::Range<TimeCode>, String> {
    let source_range = args.source_ranges.as_ref().map_or_else(
        || TimeCode::ZERO..asset.duration,
        |ranges| ranges[index].start..ranges[index].end,
    );
    if source_range.start < TimeCode::ZERO
        || source_range.end > asset.duration
        || source_range.start >= source_range.end
    {
        return Err(format!(
            "asset {} source range {}..{} must be non-empty and within 0..{}",
            asset.id, source_range.start.0, source_range.end.0, asset.duration.0
        ));
    }
    Ok(source_range)
}

fn dialogue_pacing_settings(
    args: &DialogueAssemblyPlanArgs,
) -> Result<DialoguePacingSettings, &'static str> {
    let retained = args.retained_pause_source_frames.unwrap_or(TimeCode::ZERO);
    let padding = args.filler_padding_source_frames.unwrap_or(TimeCode::ZERO);
    let maximum_filler_bridge_pause = args.maximum_filler_bridge_pause_source_frames;
    if retained < TimeCode::ZERO
        || padding < TimeCode::ZERO
        || maximum_filler_bridge_pause.is_some_and(|pause| pause < TimeCode::ZERO)
    {
        return Err(
            "retained_pause_source_frames, filler_padding_source_frames, and maximum_filler_bridge_pause_source_frames must be non-negative",
        );
    }
    Ok(DialoguePacingSettings {
        retained_pause: retained,
        filler_padding: padding,
        maximum_filler_bridge_pause,
    })
}

fn dialogue_selection(
    ranges: &[std::ops::Range<TimeCode>],
    transcript: &AssetTranscript,
    silences: &AssetSilences,
    pacing: DialoguePacingSettings,
    minimum_silence_source_frames: TimeCode,
) -> serde_json::Value {
    serde_json::json!({
        "asset_id": transcript.asset,
        "kept_source_ranges": ranges,
        "filler_bridges": dialogue_filler_bridges(
            transcript,
            silences,
            pacing.maximum_filler_bridge_pause,
            minimum_silence_source_frames,
        ),
    })
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub(super) struct DialogueFillerBridge {
    pub(super) previous_word: String,
    pub(super) next_word: String,
    pub(super) source_start: TimeCode,
    pub(super) source_end: TimeCode,
    pub(super) cut_start: TimeCode,
    pub(super) cut_end: TimeCode,
    pub(super) available_pause_source_frames: TimeCode,
    pub(super) maximum_pause_source_frames: TimeCode,
    pub(super) maximum_contiguous_pause_source_frames: TimeCode,
    pub(super) retained_pause_source_frames: TimeCode,
    pub(super) measurement: &'static str,
}

pub(super) fn dialogue_filler_bridges(
    transcript: &AssetTranscript,
    silences: &AssetSilences,
    maximum_pause: Option<TimeCode>,
    minimum_silence_source_frames: TimeCode,
) -> Vec<DialogueFillerBridge> {
    let Some(maximum_pause) = maximum_pause else {
        return Vec::new();
    };
    let mut bridges = Vec::new();
    let mut index = 0;
    while index < transcript.words.len() {
        if !is_filler_word(&transcript.words[index].text) {
            index += 1;
            continue;
        }
        let first_filler = index;
        while index < transcript.words.len() && is_filler_word(&transcript.words[index].text) {
            index += 1;
        }
        let next_non_filler = index;
        let Some(previous_non_filler) = first_filler.checked_sub(1) else {
            continue;
        };
        let Some(next) = transcript.words.get(next_non_filler) else {
            continue;
        };
        let previous = &transcript.words[previous_non_filler];
        let first = &transcript.words[first_filler];
        let last = &transcript.words[next_non_filler - 1];
        if previous.source_end > first.source_start
            || first.source_start > last.source_end
            || last.source_end > next.source_start
        {
            continue;
        }
        let left_silence = silences
            .spans
            .iter()
            .filter(|span| {
                span.source_start < first.source_start && span.source_end >= first.source_start
            })
            .min_by_key(|span| span.source_start);
        let right_silence = silences
            .spans
            .iter()
            .filter(|span| {
                span.source_start <= last.source_end && span.source_end > last.source_end
            })
            .max_by_key(|span| span.source_end);
        let (bridge_start, bridge_end, left_available, right_available, measurement) =
            if let (Some(left_silence), Some(right_silence)) = (left_silence, right_silence) {
                (
                    left_silence.source_start,
                    right_silence.source_end,
                    first
                        .source_start
                        .0
                        .saturating_sub(left_silence.source_start.0),
                    right_silence.source_end.0.saturating_sub(last.source_end.0),
                    "acoustic_silence",
                )
            } else {
                (
                    previous.source_end,
                    next.source_start,
                    first.source_start.0.saturating_sub(previous.source_end.0),
                    next.source_start.0.saturating_sub(last.source_end.0),
                    "transcript_bounds",
                )
            };
        let available = left_available.saturating_add(right_available);
        let maximum_contiguous = minimum_silence_source_frames.0.saturating_sub(1);
        let left_capacity = left_available.min(maximum_contiguous);
        let right_capacity = right_available.min(maximum_contiguous);
        let requested = maximum_pause.0;
        let mut left = (requested / 2).min(left_capacity);
        let mut right = requested.saturating_sub(left).min(right_capacity);
        let mut remaining = requested.saturating_sub(left).saturating_sub(right);
        let left_extra = left_capacity.saturating_sub(left).min(remaining);
        left = left.saturating_add(left_extra);
        remaining = remaining.saturating_sub(left_extra);
        let right_extra = right_capacity.saturating_sub(right).min(remaining);
        right = right.saturating_add(right_extra);
        let cut_start = TimeCode(bridge_start.0.saturating_add(left));
        let cut_end = TimeCode(bridge_end.0.saturating_sub(right));
        if cut_end <= cut_start {
            continue;
        }
        bridges.push(DialogueFillerBridge {
            previous_word: previous.text.clone(),
            next_word: next.text.clone(),
            source_start: bridge_start,
            source_end: bridge_end,
            cut_start,
            cut_end,
            available_pause_source_frames: TimeCode(available),
            maximum_pause_source_frames: maximum_pause,
            maximum_contiguous_pause_source_frames: TimeCode(maximum_contiguous),
            retained_pause_source_frames: TimeCode(left.saturating_add(right)),
            measurement,
        });
    }
    bridges
}

pub(super) fn dialogue_keep_ranges(
    asset: &MediaAsset,
    transcript: &AssetTranscript,
    silences: &AssetSilences,
    minimum_silence_source_frames: TimeCode,
    remove_fillers: bool,
    pacing: DialoguePacingSettings,
    source_range: std::ops::Range<TimeCode>,
) -> Vec<std::ops::Range<TimeCode>> {
    let bridges = if remove_fillers {
        dialogue_filler_bridges(
            transcript,
            silences,
            pacing.maximum_filler_bridge_pause,
            minimum_silence_source_frames,
        )
    } else {
        Vec::new()
    };
    let mut cuts = silences
        .spans
        .iter()
        .filter(|span| {
            span.source_end.0.saturating_sub(span.source_start.0) >= minimum_silence_source_frames.0
        })
        .flat_map(|span| {
            crate::silence::shrink_silence_span_for_cutting_with_transcript(
                *span,
                asset.fps,
                Some(&transcript.words),
            )
        })
        .flat_map(|span| subtract_dialogue_bridges(span.source_start..span.source_end, &bridges))
        .filter_map(|span| {
            let before = pacing.retained_pause.0 / 2;
            let after = pacing.retained_pause.0.saturating_sub(before);
            let start = TimeCode(span.start.0.saturating_add(before));
            let end = TimeCode(span.end.0.saturating_sub(after));
            (end > start).then_some(start..end)
        })
        .collect::<Vec<_>>();
    if remove_fillers {
        cuts.extend(
            transcript
                .words
                .iter()
                .filter(|word| is_filler_word(&word.text))
                .filter(|word| {
                    !bridges.iter().any(|bridge| {
                        word.source_start >= bridge.source_start
                            && word.source_end <= bridge.source_end
                    })
                })
                .map(|word| {
                    TimeCode(word.source_start.0.saturating_sub(pacing.filler_padding.0))
                        ..TimeCode(word.source_end.0.saturating_add(pacing.filler_padding.0))
                }),
        );
    }
    for cut in &mut cuts {
        cut.start = cut.start.clamp(source_range.start, source_range.end);
        cut.end = cut.end.clamp(source_range.start, source_range.end);
    }
    cuts.retain(|cut| cut.end > cut.start);
    let mut merged = merge_dialogue_cuts(cuts, pacing.retained_pause);
    merged.extend(
        bridges
            .iter()
            .map(|bridge| bridge.cut_start..bridge.cut_end),
    );
    let exact = merge_dialogue_cuts(merged, TimeCode::ZERO);
    let mut kept = Vec::new();
    let mut cursor = source_range.start;
    for cut in exact {
        if cut.start > cursor {
            kept.push(cursor..cut.start);
        }
        cursor = cursor.max(cut.end);
    }
    if cursor < source_range.end {
        kept.push(cursor..source_range.end);
    }
    kept
}

fn merge_dialogue_cuts(
    mut cuts: Vec<std::ops::Range<TimeCode>>,
    join_gap: TimeCode,
) -> Vec<std::ops::Range<TimeCode>> {
    cuts.sort_by_key(|cut| (cut.start, cut.end));
    let mut merged = Vec::<std::ops::Range<TimeCode>>::new();
    for cut in cuts {
        if let Some(previous) = merged.last_mut()
            && cut.start.0 <= previous.end.0.saturating_add(join_gap.0)
        {
            previous.end = previous.end.max(cut.end);
        } else {
            merged.push(cut);
        }
    }
    merged
}

fn subtract_dialogue_bridges(
    range: std::ops::Range<TimeCode>,
    bridges: &[DialogueFillerBridge],
) -> Vec<std::ops::Range<TimeCode>> {
    let mut remaining = vec![range];
    for bridge in bridges {
        let excluded = bridge.source_start..bridge.source_end;
        let mut next = Vec::new();
        for candidate in remaining {
            if excluded.end <= candidate.start || excluded.start >= candidate.end {
                next.push(candidate);
                continue;
            }
            if candidate.start < excluded.start {
                next.push(candidate.start..excluded.start.min(candidate.end));
            }
            if candidate.end > excluded.end {
                next.push(excluded.end.max(candidate.start)..candidate.end);
            }
        }
        remaining = next;
    }
    remaining
        .into_iter()
        .filter(|range| range.end > range.start)
        .collect()
}
