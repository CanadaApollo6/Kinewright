use super::{
    Arc, BTreeMap, CallToolResult, KinewrightMcp, McpError, SchemaError, TimeCode, Tool,
    ToolAnnotations, TranscriptRangeArgs, read_only_annotations, schema_object, success_structured,
};
use crate::dialogue_tools::{self as dialogue, CheckArgs, CutArgs, DialogueMeasurements, Refusal};

pub(super) fn host_admitted(request: &super::CallToolRequestParams) -> bool {
    let name = if request.name == "invoke_capability" {
        request
            .arguments
            .as_ref()
            .and_then(|a| a.get("name"))
            .and_then(serde_json::Value::as_str)
    } else {
        Some(request.name.as_ref())
    };
    name.is_some_and(|name| {
        matches!(
            name,
            "get_timeline_transcript" | "cut_dialogue" | "check_dialogue_cut"
        )
    })
}
pub(super) fn tools() -> Vec<Tool> {
    vec![
    Tool::new("cut_dialogue","Remove word-address ranges or tighten pauses using measured quiet. Addresses are (clip,index) from get_timeline_transcript; supply its revision and clip binding keys. Atomic, one Undo, existing destructive-edit consent and sync-lock ripple. Unit speed only. Overrule explicitly supplies exact source microseconds and bypasses only acoustic/run-together policy; keep-side rounding and minimum cut still apply. The cut field uses expected_revision; edge/join frames use the returned revision. Edge tuples: [project_frame,recording,[source_us_numerator,denominator],quiet|dip|boundary|overrule,optional [word,start|end,intrusion_us]].",schema_object::<CutArgs>()).with_annotations(ToolAnnotations::new().read_only(false).destructive(true).idempotent(false)),
    Tool::new("check_dialogue_cut","Read the current unit-speed dialogue track. Independent flags: edge in sound, short air, room-tone jump, dip intrusion; repeats, long pauses and sounding transcript holes are warnings. Default flags only; detail all includes information. Not-checked clips are explicit. Other audible clips are counted, never mixed or judged. Hole level is [RMS,peak] centidB over its source location; group indexes hole_groups. Playback range narrows evidence without creating clip edges. No score match counts are presented without their error quantiles.",schema_object::<CheckArgs>()).with_annotations(read_only_annotations()),
]
}
fn reply(value: serde_json::Value, failed: bool) -> CallToolResult {
    let mut result = success_structured(
        if failed {
            "Dialogue request refused; nothing applied"
        } else {
            "Dialogue evidence"
        },
        value,
    );
    result.is_error = Some(failed);
    result
}
fn refused(reason: &Refusal) -> CallToolResult {
    reply(
        serde_json::json!({"status":"refused","reason":reason}),
        true,
    )
}
impl KinewrightMcp {
    pub(super) fn dialogue_enabled(&self) -> bool {
        dialogue::SERVE_DIALOGUE_TOOLS
            || cfg!(test)
                && self
                    .dialogue_measurements
                    .read()
                    .is_ok_and(|slot| slot.is_some())
    }
    pub(super) fn dialogue_provider(&self) -> Result<Arc<dyn DialogueMeasurements>, Refusal> {
        self.dialogue_measurements
            .read()
            .map_err(|_| dialogue::request("measurement provider lock failed"))?
            .as_ref()
            .map(Arc::clone)
            .ok_or(Refusal::NotMeasured)
    }
    pub(super) fn dialogue_catalog(&self) -> Result<Vec<Tool>, SchemaError> {
        let mut result = Self::capability_tools()?;
        if !dialogue::SERVE_DIALOGUE_TOOLS && self.dialogue_enabled() {
            result.extend(tools());
        }
        Ok(result)
    }
    pub(super) fn check_dialogue(&self, args: &CheckArgs) -> Result<CallToolResult, McpError> {
        let (revision, doc) = self.snapshot()?;
        let result = self
            .dialogue_provider()
            .and_then(|provider| dialogue::check(&doc, args, provider.as_ref()));
        Ok(match result {
            Ok(check) => {
                let mut result = reply(
                    serde_json::json!({"revision":revision,"check":check}),
                    false,
                );
                dialogue::bound_check(
                    &mut result,
                    matches!(args.detail, dialogue::CheckDetail::All),
                );
                result
            }
            Err(reason) => refused(&reason),
        })
    }
    pub(super) fn cut_dialogue(&self, args: &CutArgs) -> Result<CallToolResult, McpError> {
        let (revision, before) = self.snapshot()?;
        if revision != args.expected_revision {
            return Ok(refused(&Refusal::Revision));
        }
        let provider = match self.dialogue_provider() {
            Ok(p) => p,
            Err(e) => return Ok(refused(&e)),
        };
        let plan = match dialogue::plan(&before, args, provider.as_ref()) {
            Ok(p) => p,
            Err(e) => return Ok(refused(&e)),
        };
        if !plan.valid {
            return Ok(reply(
                serde_json::json!({"status":"refused","removals":plan.outcomes}),
                true,
            ));
        }
        let mut checked = None;
        let validate = || -> Result<(), Refusal> {
            let current = self.dialogue_provider()?;
            for (id, value) in &plan.bindings {
                let asset = before.asset(*id).ok_or(Refusal::Identity)?;
                let now = dialogue::bound(current.as_ref(), asset)?;
                if now.binding() != value.binding() {
                    return Err(Refusal::BindingChanged);
                }
            }
            Ok(())
        };
        let result = self.apply_edit_plan_observed(
            args.expected_revision,
            &plan.operations,
            || validate().err().map(|reason| refused(&reason)),
            |new_revision, doc| {
                let retained: Vec<_> = plan
                    .changes
                    .positions
                    .iter()
                    .map(|(&track, positions)| {
                        let positions: Vec<_> = positions.iter().copied().collect();
                        match dialogue::check_created(doc, track, &positions, &plan.bindings) {
                            Ok(check) => serde_json::json!({"track":track.0,"check":check}),
                            Err(reason) => {
                                serde_json::json!({"track":track.0,"not_checked":reason})
                            }
                        }
                    })
                    .collect();
                checked = Some((
                    new_revision,
                    dialogue::check_created(doc, plan.track, &plan.created, &plan.bindings),
                    retained,
                ));
            },
        )?;
        if result.is_error == Some(true) {
            // Validation already supplied its typed refusal at the MCP boundary.
            if result.structured_content.is_some() {
                return Ok(result);
            }
            let cause = result
                .content
                .iter()
                .filter_map(|b| b.as_text().map(|t| t.text.as_str()))
                .collect::<Vec<_>>()
                .join("\n");
            return Ok(reply(
                serde_json::json!({"status":"refused","reason":{"code":"edit_refused","cause":cause},"removals":plan.outcomes}),
                true,
            ));
        }
        // The observer receives Core's committed document, before any later edit
        // can replace the live snapshot. A failed readback never lies about apply.
        let (revision, check, retained) = checked.expect("successful edit has a committed event");
        Ok(match check {
            Ok(joins) => reply(
                serde_json::json!({"status":"applied","from_revision":args.expected_revision,"revision":revision,"removals":plan.outcomes,"joins":joins,"retained_edges":retained,"extent_changes":plan.changes}),
                false,
            ),
            Err(reason) => reply(
                serde_json::json!({"status":"applied","from_revision":args.expected_revision,"revision":revision,"removals":plan.outcomes,"check_failed":reason,"retained_edges":retained,"extent_changes":plan.changes}),
                false,
            ),
        })
    }
    pub(super) fn bound_transcript(
        &self,
        requested: Option<TranscriptRangeArgs>,
    ) -> Result<CallToolResult, McpError> {
        let (revision, doc) = self.snapshot()?;
        let range = requested.map_or(TimeCode::ZERO..doc.duration, |r| r.start..r.end);
        if range.start.0 < 0 || range.start >= range.end || range.end > doc.duration {
            return Ok(refused(&dialogue::request(
                "transcript playback range is outside the document",
            )));
        }
        let provider = match self.dialogue_provider() {
            Ok(p) => p,
            Err(e) => return Ok(refused(&e)),
        };
        let played = match dialogue::spans(&doc) {
            Ok(p) => p,
            Err(e) => return Ok(refused(&e)),
        };
        let mut segments = Vec::new();
        let mut unavailable = Vec::new();
        let mut values = BTreeMap::new();
        for track in &doc.tracks {
            for clip in &track.clips {
                if doc.track_audible(track.id)
                    && clip.content.is_media()
                    && clip.speed_percent != 100
                {
                    unavailable.push(serde_json::json!({"clip":clip.id.0,"reason":Refusal::Retimed{clip:clip.id.0}}));
                }
            }
        }
        for span in played {
            let result: Result<(), Refusal> = (|| {
                if !values.contains_key(&span.asset) {
                    let value = dialogue::cached(
                        provider.as_ref(),
                        doc.asset(span.asset).ok_or(Refusal::Identity)?,
                        &values,
                    )?;
                    values.insert(span.asset, value);
                }
                let value = &values[&span.asset];
                let mut current: Option<(usize, Vec<serde_json::Value>)> = None;
                let mut push = |first, words| {
                    segments.push(serde_json::json!({"clip":span.clip.0,"track":span.track.0,"binding":value.binding(),"stream":value.stream(),"recogniser":value.recogniser_key(),"first":first,"words":words}));
                };
                for (index, word) in value.dialogue().words().iter().enumerate() {
                    use kinewright_core::ExactMicros;
                    let word_start = ExactMicros::integer(word.start_micros).max(span.source.start);
                    let word_end = ExactMicros::integer(word.end_micros).min(span.source.end);
                    if word_start >= word_end {
                        continue;
                    }
                    let project = |at: ExactMicros| -> Result<i64, Refusal> {
                        let grid =
                            kinewright_core::dialogue::FrameGrid::new(span.source.start, doc.fps)
                                .map_err(|e| dialogue::request(e.to_string()))?;
                        let frame = grid
                            .frame_at(at, kinewright_core::FrameRounding::Nearest)
                            .map_err(|e| dialogue::request(e.to_string()))?
                            .0;
                        Ok(frame + span.project.start.0)
                    };
                    let start = project(word_start)?;
                    let end = project(word_end)?;
                    if end <= range.start.0 || start >= range.end.0 {
                        continue;
                    }
                    if current
                        .as_ref()
                        .is_some_and(|(first, words)| first + words.len() != index)
                    {
                        let (first, words) = current.take().expect("present");
                        push(first, words);
                    }
                    current
                        .get_or_insert_with(|| (index, Vec::new()))
                        .1
                        .push(serde_json::json!([
                            &word.text,
                            word.start_micros,
                            word.end_micros,
                            start,
                            end
                        ]));
                }
                if let Some((first, words)) = current {
                    push(first, words);
                }
                Ok(())
            })();
            if let Err(reason) = result {
                unavailable.push(serde_json::json!({"clip":span.clip.0,"reason":reason}));
            }
        }
        Ok(reply(
            serde_json::json!({"revision":revision,"word_address":"(clip, first + offset)","word_columns":["text","source_start_us","source_end_us","project_start","project_end"],"segments":segments,"recording_level_columns":["speech_rms_cdb","noise_floor_cdb"],"recording_levels":values.values().map(|v| {let levels=v.dialogue().profile().levels();(v.binding(),[levels.speech_rms_hundredths,levels.noise_floor_hundredths])}).collect::<BTreeMap<_,_>>(),"not_measured":unavailable}),
            false,
        ))
    }
}
