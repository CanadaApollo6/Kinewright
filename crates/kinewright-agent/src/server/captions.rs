//! Caption inspection, correction planning, styled captions, and the QA report.

use super::*;

impl KinewrightMcp {
    pub(super) fn caption_presets() -> CallToolResult {
        let presets = CaptionPreset::ALL.map(|preset| {
            let title = preset.title("Example caption");
            serde_json::json!({
                "id": preset.as_str(),
                "font_size_token": title.font_size_token,
                "color_token": title.color_token,
                "position": title.position.as_str(),
                "background_scrim": title.background_scrim,
                "motions": CaptionMotion::ALL.map(CaptionMotion::as_str),
            })
        });
        success_text(
            serde_json::to_string_pretty(&presets)
                .unwrap_or_else(|error| format!("could not serialize presets: {error}")),
        )
    }

    pub(super) fn captions(&self, args: CaptionListArgs) -> Result<CallToolResult, McpError> {
        let (revision, document) = self.snapshot()?;
        let range = args.range.map(|range| range.start..range.end);
        if let Some(range) = &range
            && (range.start < TimeCode::ZERO
                || range.end <= range.start
                || range.end > document.duration)
        {
            return Ok(error_text(format!(
                "caption range {}..{} is outside project range 0..{}",
                range.start.0, range.end.0, document.duration.0
            )));
        }
        let offset = args.offset.unwrap_or(0);
        let limit = args.limit.unwrap_or(50).clamp(1, 200);
        let mut captions = Vec::new();
        for track in &document.tracks {
            for clip in &track.clips {
                let ClipContent::Title(title) = &clip.content else {
                    continue;
                };
                let Some(preset) = title.caption_preset else {
                    continue;
                };
                let Ok(duration) = document.clip_duration(clip) else {
                    continue;
                };
                let Some(end) = clip.timeline_start.checked_add(duration) else {
                    continue;
                };
                if range.as_ref().is_some_and(|requested| {
                    end <= requested.start || clip.timeline_start >= requested.end
                }) {
                    continue;
                }
                captions.push((track.id, clip, title, preset, end));
            }
        }
        captions.sort_by_key(|(_, clip, _, _, _)| (clip.timeline_start, clip.id));
        let total = captions.len();
        let page = captions
            .into_iter()
            .skip(offset)
            .take(limit)
            .map(|(track, clip, title, preset, end)| {
                serde_json::json!({
                    "clip_id": clip.id,
                    "track_id": track,
                    "start_frame": clip.timeline_start,
                    "end_frame": end,
                    "text": title.text,
                    "preset": preset,
                })
            })
            .collect::<Vec<_>>();
        let next_offset = (offset + page.len() < total).then_some(offset + page.len());
        let mut rendered = format!(
            "timeline_revision={revision} captions_total={total} offset={offset} returned={} next_offset={next_offset:?}",
            page.len()
        );
        for caption in &page {
            let _ = write!(
                rendered,
                "\nclip={} track={} range={}..{} preset={} text={:?}",
                caption["clip_id"],
                caption["track_id"],
                caption["start_frame"],
                caption["end_frame"],
                caption["preset"].as_str().unwrap_or("unknown"),
                caption["text"].as_str().unwrap_or_default(),
            );
        }
        Ok(success_structured(
            rendered,
            serde_json::json!({
                "timeline_revision": revision,
                "total": total,
                "offset": offset,
                "limit": limit,
                "next_offset": next_offset,
                "captions": page,
            }),
        ))
    }

    pub(super) fn plan_caption_corrections(
        &self,
        args: CaptionCorrectionPlanArgs,
    ) -> Result<CallToolResult, McpError> {
        let (revision, document) = self.snapshot()?;
        if args.expected_revision != revision {
            return Ok(revision_conflict_text(args.expected_revision, revision));
        }
        if args.corrections.is_empty() || args.corrections.len() > 100 {
            return Ok(error_text(
                "caption correction plan requires between 1 and 100 corrections",
            ));
        }
        let mut seen = BTreeSet::new();
        let mut operations = Vec::with_capacity(args.corrections.len());
        for correction in args.corrections {
            if !seen.insert(correction.clip_id) {
                return Ok(error_text(format!(
                    "caption clip {} appears more than once",
                    correction.clip_id
                )));
            }
            if correction.text.trim().is_empty() {
                return Ok(error_text(format!(
                    "caption clip {} replacement text is empty",
                    correction.clip_id
                )));
            }
            let Some(clip) = document.clip(correction.clip_id) else {
                return Ok(error_text(format!(
                    "caption clip {} does not exist",
                    correction.clip_id
                )));
            };
            if !matches!(
                &clip.content,
                ClipContent::Title(title) if title.caption_preset.is_some()
            ) {
                return Ok(error_text(format!(
                    "clip {} is not a generated caption",
                    correction.clip_id
                )));
            }
            operations.push(Operation::SetTitleParam {
                clip: correction.clip_id,
                name: "text".to_owned(),
                value: ParamValue::Text(correction.text),
            });
        }
        let plan = match self.prepare_operations(revision, &document, operations) {
            Ok(plan) => plan,
            Err(error) => {
                return Ok(error_text(format!(
                    "caption corrections are invalid: {error}"
                )));
            }
        };
        Ok(success_structured(
            format!(
                "prepared {} caption correction(s) as edit plan {}; inspect the preview, then commit it at timeline revision {revision}",
                plan.preview.operation_count, plan.id,
            ),
            serde_json::json!({
                "timeline_revision": revision,
                "prepared_edit_plan": {
                    "plan_id": plan.id,
                    "expected_revision": revision,
                    "preview": plan.preview,
                },
            }),
        ))
    }

    pub(super) fn add_styled_captions(
        &self,
        args: &StyledCaptionsArgs,
    ) -> Result<CallToolResult, McpError> {
        let expected_revision = args.expected_revision;
        let (actual_revision, document) = self.snapshot()?;
        if expected_revision != actual_revision {
            return Ok(revision_conflict_text(expected_revision, actual_revision));
        }
        if args.intent == CaptionIntent::EditedReadable && args.script.is_none() {
            return Ok(error_text(
                "edited_readable captions require an explicit authored script",
            ));
        }
        let position = match caption_position(args.position, args.subject_y_percent) {
            Ok(position) => Some(position),
            Err(error) => return Ok(error_text(error)),
        };
        let words = self
            .analysis
            .timeline_transcript(&document, None)
            .map_err(|error| McpError::internal_error(error.to_string(), None))?;
        let words = dedup_timeline_words(words);
        let mut cues = caption_cues(&words, document.fps);
        clamp_caption_cues_to_duration(&mut cues, document.duration);
        if let Some(script) = args.script.as_deref() {
            cues = match authored_caption_cues(&cues, script) {
                Ok(cues) => cues,
                Err(error) => return Ok(error_text(error.to_string())),
            };
        }
        let operations = match animated_caption_operations_at(
            &document,
            &cues,
            args.preset,
            args.motion,
            position,
        ) {
            Ok(operations) => operations,
            Err(error) => return Ok(error_text(error.to_string())),
        };
        self.apply_edit_plan(expected_revision, &operations)
    }

    pub(super) fn qa_report(&self) -> Result<CallToolResult, McpError> {
        let (revision, document) = self.snapshot()?;
        let report = qa_document(&document);
        let json = serde_json::to_string_pretty(&serde_json::json!({
            "timeline_revision": revision,
            "export_ready": report.export_ready(),
            "error_count": report.count(kinewright_core::QaSeverity::Error),
            "warning_count": report.count(kinewright_core::QaSeverity::Warning),
            "info_count": report.count(kinewright_core::QaSeverity::Info),
            "issues": report.issues,
        }))
        .map_err(|error| McpError::internal_error(error.to_string(), None))?;
        Ok(success_text(json))
    }
}

pub(super) fn caption_position(
    explicit: Option<TitlePosition>,
    subject_y_percent: Option<u8>,
) -> Result<TitlePosition, &'static str> {
    if subject_y_percent.is_some_and(|value| value > 100) {
        return Err("caption subject_y_percent must be between 0 and 100");
    }
    Ok(explicit.unwrap_or_else(|| {
        if subject_y_percent.is_some_and(|subject_y| subject_y >= 60) {
            TitlePosition::Top
        } else {
            TitlePosition::LowerThird
        }
    }))
}

pub(super) fn clamp_caption_cues_to_duration(cues: &mut Vec<CaptionCue>, duration: TimeCode) {
    for cue in &mut *cues {
        cue.end = cue.end.min(duration);
    }
    cues.retain(|cue| cue.start < duration && cue.end > cue.start);
}
