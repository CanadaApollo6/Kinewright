//! Read-only asset and timeline analysis inspectors.

use super::planning::beat_montage_analysis_state;
use super::*;

impl KinewrightMcp {
    pub(super) fn asset_transcript(&self, asset_id: AssetId) -> Result<CallToolResult, McpError> {
        let document = self.document()?;
        let Some(asset) = document.asset(asset_id) else {
            return Ok(error_text(format!("asset {asset_id} does not exist")));
        };
        let mut status = self.analysis.transcript_status(asset);
        if status == TranscriptStatus::NotRequested {
            self.analysis.request_transcription(asset.clone());
            status = self.analysis.transcript_status(asset);
        }
        Ok(success_text(render_asset_transcript(asset_id, &status)))
    }

    pub(super) fn asset_transcripts(
        &self,
        asset_ids: &[AssetId],
    ) -> Result<CallToolResult, McpError> {
        if asset_ids.is_empty() || asset_ids.len() > 32 {
            return Ok(error_text("get_transcripts requires 1..=32 asset_ids"));
        }
        let unique = asset_ids.iter().copied().collect::<BTreeSet<_>>();
        if unique.len() != asset_ids.len() {
            return Ok(error_text("get_transcripts asset_ids must be unique"));
        }
        let document = self.document()?;
        let mut rendered = Vec::with_capacity(asset_ids.len());
        for asset_id in asset_ids {
            let Some(asset) = document.asset(*asset_id) else {
                return Ok(error_text(format!("asset {asset_id} does not exist")));
            };
            let mut status = self.analysis.transcript_status(asset);
            if status == TranscriptStatus::NotRequested {
                self.analysis.request_transcription(asset.clone());
                status = self.analysis.transcript_status(asset);
            }
            rendered.push(render_asset_transcript(*asset_id, &status));
        }
        Ok(success_text(rendered.join("\n")))
    }

    pub(super) fn asset_silences(
        &self,
        asset_id: AssetId,
        requested_minimum: Option<TimeCode>,
    ) -> Result<CallToolResult, McpError> {
        let document = self.document()?;
        let Some(asset) = document.asset(asset_id) else {
            return Ok(error_text(format!("asset {asset_id} does not exist")));
        };
        let minimum = requested_minimum.unwrap_or(TimeCode(DEFAULT_MINIMUM_SILENCE_FRAMES));
        if minimum <= TimeCode::ZERO {
            return Ok(error_text("min_duration_frames must be positive"));
        }
        let mut status = self.analysis.silence_status(asset);
        if status == SilenceStatus::NotRequested {
            self.analysis.request_silence_detection(asset.clone());
            status = self.analysis.silence_status(asset);
        }
        let transcript = match self.analysis.transcript_status(asset) {
            TranscriptStatus::Ready(transcript) => Some(transcript),
            _ => None,
        };
        Ok(success_text(render_asset_silences(
            asset_id,
            &status,
            minimum,
            transcript.as_deref(),
        )))
    }

    pub(super) fn asset_scene_changes(
        &self,
        asset_id: AssetId,
        requested_minimum: Option<f64>,
    ) -> Result<CallToolResult, McpError> {
        let document = self.document()?;
        let Some(asset) = document.asset(asset_id) else {
            return Ok(error_text(format!("asset {asset_id} does not exist")));
        };
        let minimum = match requested_minimum {
            Some(value) => match confidence_to_basis_points(value) {
                Ok(value) => value,
                Err(error) => return Ok(error_text(error)),
            },
            None => DEFAULT_SCENE_CONFIDENCE_BASIS_POINTS,
        };
        let mut status = self.analysis.scene_status(asset);
        if status == SceneStatus::NotRequested {
            self.analysis.request_scene_detection(asset.clone());
            status = self.analysis.scene_status(asset);
        }
        Ok(success_text(render_asset_scene_changes(
            asset_id, &status, minimum,
        )))
    }

    pub(super) fn asset_beats(
        &self,
        asset_id: AssetId,
        requested_minimum: Option<f64>,
    ) -> Result<CallToolResult, McpError> {
        let document = self.document()?;
        let Some(asset) = document.asset(asset_id) else {
            return Ok(error_text(format!("asset {asset_id} does not exist")));
        };
        let minimum = match requested_minimum {
            Some(value) => match percentage_to_basis_points(value, "min_strength") {
                Ok(value) => value,
                Err(error) => return Ok(error_text(error)),
            },
            None => DEFAULT_BEAT_STRENGTH_BASIS_POINTS,
        };
        let mut status = self.analysis.beat_status(asset);
        if status == BeatStatus::NotRequested {
            self.analysis.request_beat_detection(asset.clone());
            status = self.analysis.beat_status(asset);
        }
        Ok(render_asset_beats(asset_id, &status, minimum))
    }

    pub(super) fn analysis_status(&self, asset_id: AssetId) -> Result<CallToolResult, McpError> {
        let document = self.document()?;
        let Some(asset) = document.asset(asset_id) else {
            return Ok(error_text(format!("asset {asset_id} does not exist")));
        };
        let jobs = self.analysis.analysis_jobs(asset);
        Ok(success_structured(
            format!(
                "asset {asset_id} analysis jobs {}",
                serde_json::to_string(&jobs).unwrap_or_else(|_| "[]".to_owned())
            ),
            serde_json::json!({"asset_id": asset_id.0, "jobs": jobs}),
        ))
    }

    pub(super) fn request_analysis(
        &self,
        asset_id: AssetId,
        requested: &[AnalysisKind],
    ) -> Result<CallToolResult, McpError> {
        let document = self.document()?;
        let Some(asset) = document.asset(asset_id).cloned() else {
            return Ok(error_text(format!("asset {asset_id} does not exist")));
        };
        let kinds = if requested.is_empty() {
            AnalysisKind::ALL.as_slice()
        } else {
            requested
        };
        for kind in kinds {
            match kind {
                AnalysisKind::Transcript => self.analysis.request_transcription(asset.clone()),
                AnalysisKind::Silence => self.analysis.request_silence_detection(asset.clone()),
                AnalysisKind::Scene => self.analysis.request_scene_detection(asset.clone()),
                AnalysisKind::Beat => self.analysis.request_beat_detection(asset.clone()),
            }
        }
        self.analysis_status(asset_id)
    }

    pub(super) fn cancel_analysis(
        &self,
        asset_id: AssetId,
        kind: AnalysisKind,
    ) -> Result<CallToolResult, McpError> {
        let document = self.document()?;
        let Some(asset) = document.asset(asset_id) else {
            return Ok(error_text(format!("asset {asset_id} does not exist")));
        };
        let cancelled = self.analysis.cancel_analysis(asset, kind);
        let jobs = self.analysis.analysis_jobs(asset);
        Ok(success_structured(
            format!("asset {asset_id} analysis kind={kind:?} cancelled={cancelled}"),
            serde_json::json!({
                "asset_id": asset_id.0,
                "kind": kind,
                "cancelled": cancelled,
                "jobs": jobs,
            }),
        ))
    }

    pub(super) fn timeline_transcript(
        &self,
        requested: Option<TranscriptRangeArgs>,
    ) -> Result<CallToolResult, McpError> {
        let document = self.document()?;
        let range = requested.map_or(TimeCode::ZERO..document.duration, |range| {
            range.start..range.end
        });
        if range.start < TimeCode::ZERO || range.end <= range.start || range.end > document.duration
        {
            return Ok(error_text(format!(
                "timeline transcript range {}..{} is outside project range 0..{}",
                range.start.0, range.end.0, document.duration.0
            )));
        }
        for asset in &document.media_pool {
            if self.analysis.transcript_status(asset) == TranscriptStatus::NotRequested {
                self.analysis.request_transcription(asset.clone());
            }
        }
        let words: Vec<TimelineTranscriptWord> = match self
            .analysis
            .timeline_transcript(&document, Some(range.clone()))
        {
            Ok(words) => words,
            Err(error) => return Ok(error_text(error.to_string())),
        };
        let mut rendered = render_timeline_transcript(&document, range, &words);
        for asset in &document.media_pool {
            let status = self.analysis.transcript_status(asset);
            if !matches!(
                status,
                TranscriptStatus::Ready(_) | TranscriptStatus::NoSpeech
            ) {
                rendered.push('\n');
                rendered.push_str(&render_asset_transcript(asset.id, &status));
            }
        }
        Ok(success_text(rendered))
    }

    pub(super) fn dialogue_pacing(
        &self,
        args: &DialoguePacingArgs,
    ) -> Result<CallToolResult, McpError> {
        let document = self.document()?;
        let range = validated_timeline_range(
            &document,
            args.range.as_ref().map(|range| TranscriptRangeArgs {
                start: range.start,
                end: range.end,
            }),
            "dialogue pacing",
        )?;
        let minimum = args.minimum_pause_frames.unwrap_or(TimeCode(10));
        let maximum = args.maximum_pause_frames.unwrap_or(TimeCode(40));
        let capitalization_minimum = args
            .capitalization_boundary_minimum_frames
            .unwrap_or(TimeCode(4));
        if minimum < TimeCode::ZERO || maximum < minimum || capitalization_minimum < TimeCode::ZERO
        {
            return Ok(error_text(
                "dialogue pacing requires 0 <= minimum_pause_frames <= maximum_pause_frames and a non-negative capitalization boundary minimum",
            ));
        }
        let referenced_assets = document
            .tracks
            .iter()
            .flat_map(|track| &track.clips)
            .filter(|clip| clip.content.is_media())
            .map(|clip| clip.asset)
            .collect::<BTreeSet<_>>();
        for asset in document
            .media_pool
            .iter()
            .filter(|asset| referenced_assets.contains(&asset.id))
        {
            if self.analysis.transcript_status(asset) == TranscriptStatus::NotRequested {
                self.analysis.request_transcription(asset.clone());
            }
            if self.analysis.silence_status(asset) == SilenceStatus::NotRequested {
                self.analysis.request_silence_detection(asset.clone());
            }
        }
        let words = match self
            .analysis
            .timeline_transcript(&document, Some(range.clone()))
        {
            Ok(words) => dedup_timeline_words(words),
            Err(error) => return Ok(error_text(error.to_string())),
        };
        let silences =
            match self
                .analysis
                .timeline_silences(&document, Some(range.clone()), TimeCode(1))
            {
                Ok(silences) => silences,
                Err(error) => return Ok(error_text(error.to_string())),
            };
        let pending_acoustic_assets = document
            .media_pool
            .iter()
            .filter(|asset| referenced_assets.contains(&asset.id))
            .filter(|asset| {
                !matches!(
                    self.analysis.silence_status(asset),
                    SilenceStatus::Ready(_) | SilenceStatus::NoAudio
                )
            })
            .map(|asset| asset.id.0)
            .collect::<Vec<_>>();
        let pacing =
            dialogue_pacing_gaps(&words, &silences, minimum, maximum, capitalization_minimum);
        Ok(dialogue_pacing_result(
            range,
            minimum,
            maximum,
            capitalization_minimum,
            &pacing,
            &pending_acoustic_assets,
        ))
    }

    pub(super) fn timeline_silences(
        &self,
        requested: Option<TranscriptRangeArgs>,
        requested_minimum: Option<TimeCode>,
    ) -> Result<CallToolResult, McpError> {
        let document = self.document()?;
        let range = validated_timeline_range(&document, requested, "timeline silence")?;
        let minimum = requested_minimum.unwrap_or(TimeCode(DEFAULT_MINIMUM_SILENCE_FRAMES));
        if minimum <= TimeCode::ZERO {
            return Ok(error_text("min_duration_frames must be positive"));
        }
        for asset in &document.media_pool {
            if self.analysis.silence_status(asset) == SilenceStatus::NotRequested {
                self.analysis.request_silence_detection(asset.clone());
            }
        }
        let transcripts = document
            .media_pool
            .iter()
            .filter_map(|asset| match self.analysis.transcript_status(asset) {
                TranscriptStatus::Ready(transcript) => Some((asset.id, transcript)),
                _ => None,
            })
            .collect::<BTreeMap<_, _>>();
        let spans: Vec<TimelineSilenceSpan> =
            match self
                .analysis
                .timeline_silences(&document, Some(range.clone()), minimum)
            {
                Ok(spans) => spans,
                Err(error) => return Ok(error_text(error.to_string())),
            };
        let mut rendered =
            render_timeline_silences(&document, range, &spans, &transcripts, minimum);
        for asset in &document.media_pool {
            let status = self.analysis.silence_status(asset);
            if !matches!(status, SilenceStatus::Ready(_) | SilenceStatus::NoAudio) {
                rendered.push('\n');
                rendered.push_str(&render_asset_silences(
                    asset.id,
                    &status,
                    minimum,
                    transcripts.get(&asset.id).map(Arc::as_ref),
                ));
            }
        }
        Ok(success_text(rendered))
    }

    pub(super) fn timeline_scene_changes(
        &self,
        requested: Option<TranscriptRangeArgs>,
    ) -> Result<CallToolResult, McpError> {
        let document = self.document()?;
        let range = validated_timeline_range(&document, requested, "timeline scene")?;
        for asset in &document.media_pool {
            if self.analysis.scene_status(asset) == SceneStatus::NotRequested {
                self.analysis.request_scene_detection(asset.clone());
            }
        }
        let changes: Vec<TimelineSceneChange> = match self.analysis.timeline_scene_changes(
            &document,
            Some(range.clone()),
            DEFAULT_SCENE_CONFIDENCE_BASIS_POINTS,
        ) {
            Ok(changes) => changes,
            Err(error) => return Ok(error_text(error.to_string())),
        };
        let mut rendered = render_timeline_scene_changes(&document, range, &changes);
        for asset in &document.media_pool {
            let status = self.analysis.scene_status(asset);
            if !matches!(status, SceneStatus::Ready(_) | SceneStatus::NoVideo) {
                rendered.push('\n');
                rendered.push_str(&render_asset_scene_changes(
                    asset.id,
                    &status,
                    DEFAULT_SCENE_CONFIDENCE_BASIS_POINTS,
                ));
            }
        }
        Ok(success_text(rendered))
    }

    pub(super) fn timeline_beats(
        &self,
        requested: Option<TranscriptRangeArgs>,
        requested_minimum: Option<f64>,
    ) -> Result<CallToolResult, McpError> {
        let document = self.document()?;
        let range = validated_timeline_range(&document, requested, "timeline beat")?;
        let minimum = match requested_minimum {
            Some(value) => match percentage_to_basis_points(value, "min_strength") {
                Ok(value) => value,
                Err(error) => return Ok(error_text(error)),
            },
            None => DEFAULT_BEAT_STRENGTH_BASIS_POINTS,
        };
        for asset in &document.media_pool {
            if self.analysis.beat_status(asset) == BeatStatus::NotRequested {
                self.analysis.request_beat_detection(asset.clone());
            }
        }
        let beats: Vec<TimelineBeat> =
            match self
                .analysis
                .timeline_beats(&document, Some(range.clone()), minimum)
            {
                Ok(beats) => beats,
                Err(error) => return Ok(error_text(error.to_string())),
            };
        let pending = document
            .media_pool
            .iter()
            .filter(|asset| {
                !matches!(
                    self.analysis.beat_status(asset),
                    BeatStatus::Ready(_) | BeatStatus::NoAudio
                )
            })
            .map(|asset| asset.id.0)
            .collect::<Vec<_>>();
        Ok(success_structured(
            render_timeline_beats(&document, &range, &beats, &pending),
            serde_json::json!({
                "range": {"start": range.start.0, "end": range.end.0},
                "minimum_strength_basis_points": minimum,
                "beats": beats,
                "pending_asset_ids": pending,
            }),
        ))
    }

    /// Return a compact, read-only heuristic hypothesis about one music
    /// asset's beat/bar/phrase structure. The analysis is deliberately kept
    /// separate from edit planning: it produces no operations and never
    /// changes the document or prepared-plan store.
    #[allow(clippy::too_many_lines)]
    pub(super) fn music_structure(
        &self,
        args: &MusicStructureArgs,
    ) -> Result<CallToolResult, McpError> {
        let document = self.document()?;
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
        let requested_range = args.range.as_ref().map(|range| TranscriptRangeArgs {
            start: range.start,
            end: range.end,
        });
        let range = validated_timeline_range(&document, requested_range, "music structure")?;
        if !timeline_contains_asset(&document, args.music_asset_id, &range) {
            return Ok(error_text(format!(
                "music asset {} is not present on an audio-capable timeline clip overlapping project range {}..{}",
                args.music_asset_id, range.start, range.end
            )));
        }
        let minimum_strength = match args.min_strength {
            Some(value) => match percentage_to_basis_points(value, "min_strength") {
                Ok(value) => value,
                Err(error) => return Ok(error_text(error)),
            },
            None => DEFAULT_BEAT_STRENGTH_BASIS_POINTS,
        };
        let meter_beats = args
            .meter_beats
            .unwrap_or(MUSIC_STRUCTURE_DEFAULT_METER_BEATS);
        let phrase_bars = args
            .phrase_bars
            .unwrap_or(MUSIC_STRUCTURE_DEFAULT_PHRASE_BARS);

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
        let analysis = match music_structure_analysis(
            &document,
            args.music_asset_id,
            range,
            &beats,
            &analysis_state,
            minimum_strength,
            meter_beats,
            phrase_bars,
        ) {
            Ok(analysis) => analysis,
            Err(error) => return Ok(error_text(error.to_string())),
        };
        let total_candidate_count = analysis.candidates.len();
        let omitted_ordinary_candidate_count = if args.structural_only {
            analysis
                .candidates
                .iter()
                .filter(|candidate| candidate.role == kinewright_core::MusicStructureRole::Beat)
                .count()
        } else {
            0
        };
        let candidates = if args.structural_only {
            analysis
                .candidates
                .iter()
                .copied()
                .filter(|candidate| candidate.role != kinewright_core::MusicStructureRole::Beat)
                .collect::<Vec<_>>()
        } else {
            analysis.candidates.clone()
        };
        let returned_candidate_count = candidates.len();
        let structured = serde_json::json!({
            "music_asset_id": analysis.music_asset.0,
            "range": {
                "start": analysis.timeline_range.start.0,
                "end": analysis.timeline_range.end.0,
            },
            "minimum_strength_basis_points": analysis.minimum_strength_basis_points,
            "analysis_status": "ready",
            "timeline_audio_asset_present": true,
            "heuristic": true,
            "structural_only": args.structural_only,
            "total_candidate_count": total_candidate_count,
            "returned_candidate_count": returned_candidate_count,
            "omitted_ordinary_candidate_count": omitted_ordinary_candidate_count,
            "disclaimer": "Heuristic candidates, not guaranteed music theory; validate the musical result by listening before using them to drive cuts.",
            "parameters": analysis.parameters,
            "candidates": candidates,
        });
        Ok(success_structured(
            format!(
                "heuristic music structure for asset {} in {}..{}: {} candidate onsets returned ({} total; {} ordinary omitted), inferred meter {} and phrase length {} bars; structural_only={}; candidates are not guaranteed music theory",
                analysis.music_asset,
                analysis.timeline_range.start,
                analysis.timeline_range.end,
                returned_candidate_count,
                total_candidate_count,
                omitted_ordinary_candidate_count,
                analysis.parameters.meter_beats,
                analysis.parameters.phrase_bars,
                args.structural_only,
            ),
            structured,
        ))
    }
}

fn timeline_contains_asset(
    document: &Document,
    asset_id: AssetId,
    range: &std::ops::Range<TimeCode>,
) -> bool {
    document.tracks.iter().any(|track| {
        if track.kind != TrackKind::Audio {
            return false;
        }
        track.clips.iter().any(|clip| {
            if clip.asset != asset_id || !clip.content.is_media() {
                return false;
            }
            let Some(duration) = document.clip_duration(clip).ok() else {
                return false;
            };
            let Some(end) = clip.timeline_start.checked_add(duration) else {
                return false;
            };
            clip.timeline_start < range.end && end > range.start
        })
    })
}

fn dialogue_pacing_result(
    range: std::ops::Range<TimeCode>,
    minimum: TimeCode,
    maximum: TimeCode,
    capitalization_minimum: TimeCode,
    pacing: &[DialoguePacingGap],
    pending_acoustic_assets: &[u64],
) -> CallToolResult {
    let short = pacing.iter().filter(|gap| gap.status == "short").count();
    let long = pacing.iter().filter(|gap| gap.status == "long").count();
    let target = pacing.len().saturating_sub(short).saturating_sub(long);
    let acoustic = pacing
        .iter()
        .filter(|gap| gap.measurement == "acoustic_silence")
        .count();
    let ready = pending_acoustic_assets.is_empty() && short == 0 && long == 0;
    let mut rendered = format!(
        "dialogue pacing range={}..{} boundaries={} acoustic={} target={} short={} long={} pending_acoustic_assets={:?} ready={ready}",
        range.start.0,
        range.end.0,
        pacing.len(),
        acoustic,
        target,
        short,
        long,
        pending_acoustic_assets,
    );
    for gap in pacing {
        let _ = write!(
            rendered,
            "\n{}..{} gap={} transcript_gap={} measurement={} status={} {:?} -> {:?} reason={}",
            gap.previous_end.0,
            gap.next_start.0,
            gap.pause_frames.0,
            gap.transcript_pause_frames.0,
            gap.measurement,
            gap.status,
            gap.previous_word,
            gap.next_word,
            gap.reason,
        );
    }
    success_structured(
        rendered,
        serde_json::json!({
            "range": {"start": range.start.0, "end": range.end.0},
            "target_pause_frames": {"minimum": minimum.0, "maximum": maximum.0},
            "capitalization_boundary_minimum_frames": capitalization_minimum.0,
            "summary": {
                "boundaries": pacing.len(),
                "target": target,
                "short": short,
                "long": long,
                "acoustic": acoustic,
                "pending_acoustic_asset_ids": pending_acoustic_assets,
                "ready": ready,
            },
            "gaps": pacing,
        }),
    )
}

// The validated 0..=100 percentage is intentionally rounded to integer basis points.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn confidence_to_basis_points(confidence: f64) -> Result<u16, String> {
    percentage_to_basis_points(confidence, "min_confidence")
}

fn render_asset_beats(
    asset: AssetId,
    status: &BeatStatus,
    minimum_strength_basis_points: u16,
) -> CallToolResult {
    match status {
        BeatStatus::NotRequested => {
            success_text(format!("asset {asset} beats status=not-requested"))
        }
        BeatStatus::Queued => success_text(format!("asset {asset} beats status=queued")),
        BeatStatus::Hashing => success_text(format!("asset {asset} beats status=hashing")),
        BeatStatus::Analyzing { progress_percent } => success_text(progress_percent.map_or_else(
            || format!("asset {asset} beats status=analyzing"),
            |progress| format!("asset {asset} beats status=analyzing progress={progress}%"),
        )),
        BeatStatus::NoAudio => success_text(format!("asset {asset} beats: no audio stream")),
        BeatStatus::Cancelled => success_text(format!("asset {asset} beats status=cancelled")),
        BeatStatus::Failed(error) => {
            error_text(format!("asset {asset} beats status=failed error={error:?}"))
        }
        BeatStatus::Ready(beats) => {
            let selected = beats
                .beats
                .iter()
                .copied()
                .filter(|beat| beat.strength_basis_points >= minimum_strength_basis_points)
                .collect::<Vec<_>>();
            let mut output = format!(
                "asset {asset} beats fps={}/{} bpm={:.3} min_strength={:.2}% onsets={}\n",
                beats.source_fps.numerator(),
                beats.source_fps.denominator(),
                f64::from(beats.estimated_bpm_milli) / 1_000.0,
                f64::from(minimum_strength_basis_points) / 100.0,
                selected.len()
            );
            for beat in &selected {
                let _ = writeln!(
                    output,
                    "{}f strength={:.2}%",
                    beat.source_frame.0,
                    f64::from(beat.strength_basis_points) / 100.0
                );
            }
            output.pop();
            success_structured(
                output,
                serde_json::json!({
                    "asset_id": asset.0,
                    "source_fps": beats.source_fps,
                    "estimated_bpm_milli": beats.estimated_bpm_milli,
                    "minimum_strength_basis_points": minimum_strength_basis_points,
                    "beats": selected,
                }),
            )
        }
    }
}

fn render_timeline_beats(
    document: &Document,
    range: &std::ops::Range<TimeCode>,
    beats: &[TimelineBeat],
    pending: &[u64],
) -> String {
    let mut output = format!(
        "timeline beats range={}..{} fps={}/{} onsets={} pending_assets={pending:?}\n",
        range.start.0,
        range.end.0,
        document.fps.numerator(),
        document.fps.denominator(),
        beats.len()
    );
    for beat in beats {
        let _ = writeln!(
            output,
            "clip={} asset={} project={}f source={}f strength={:.2}% bpm={:.3}",
            beat.clip,
            beat.asset,
            beat.project_frame.0,
            beat.source_frame.0,
            f64::from(beat.strength_basis_points) / 100.0,
            f64::from(beat.estimated_bpm_milli) / 1_000.0,
        );
    }
    output.pop();
    output
}
