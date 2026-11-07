//! Revision-bound read context. These values describe evidence, never edit authority.
use super::{HostCommandOwner, HostError, HostRefusal};
use crate::orchestration::orchestration_records::{
    AuthorityBinding, BranchBinding, OrchestrationCommand, OrchestrationEnvelope,
    OrchestrationPayload, RunId, RunProjection, TaskId, TaskRecord, TaskResult,
};
use kinewright_core::{ClipId, Command, Document, Event, Query, QueryResult, TimelineRevision};
use kinewright_project::bundle_records::{ObjectRef, RecordKind};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{collections::BTreeSet, sync::Arc};

pub(super) const PAGE_BYTES: usize = 24 * 1024;
const OVERVIEW_BYTES: usize = 8 * 1024;
const EXPANSIONS: usize = 8;
#[cfg(any(target_os = "linux", test))]
const CONTRACT: &str = "Context is read-only. Omitted or unknown facts are not success. Expand dependencies and neighbouring transcript/media before deciding. Reject or replan unmet notes, conflicts, failed or unknown proofs. A page is at most 24 KiB; eight expansions per task. evidence_capacity requires replanning, never guessing. Merge only the exact durable result, never a summary.";
fn refused(reason: &str) -> HostError {
    HostRefusal::new("task_context", reason).into()
}
fn capacity() -> HostError {
    HostRefusal::new(
        "evidence_capacity",
        "Evidence capacity exhausted; replan without guessing.",
    )
    .into()
}

#[cfg(any(target_os = "linux", test))]
pub(crate) fn application_prefix(
    role: crate::orchestration::orchestration_records::Role,
) -> Result<Vec<u8>, HostError> {
    let manifest =
        crate::server::task_manifest_identity(role).map_err(|_| refused("manifest unavailable"))?;
    Ok(format!("{{\"instructions\":{},\"task_role\":{},\"manifest_sha256\":{},\"context_contract\":{},\"data\":",
        serde_json::to_string(super::codex_instructions::CODEX_INSTRUCTIONS)?,
        serde_json::to_string(&role)?, serde_json::to_string(&manifest)?, serde_json::to_string(CONTRACT)?).into_bytes())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(super) enum EvidenceKind {
    Timeline {
        #[serde(default)]
        clips: Vec<ClipId>,
    },
    Reference {
        reference: ObjectRef,
    },
    Transcript {
        reference: ObjectRef,
        start: usize,
        end: usize,
    },
    Audio {
        clip: ClipId,
        source_frame: i64,
    },
    Frame {
        clip: ClipId,
        source_frame: i64,
    },
    Preview {
        reference: ObjectRef,
    },
    Proof {
        reference: ObjectRef,
    },
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct EvidenceRead {
    pub revision: TimelineRevision,
    pub evidence: EvidenceKind,
    #[serde(default)]
    pub cursor: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ChildRead {
    pub run: RunId,
    pub child: TaskId,
    pub reference: ObjectRef,
    #[serde(deserialize_with = "Option::deserialize")]
    pub source: Option<BranchBinding>,
    #[serde(default)]
    pub cursor: Option<String>,
}

fn clip_rows(document: &Document, seeds: &[ClipId]) -> Vec<Value> {
    let all = document
        .tracks
        .iter()
        .flat_map(|t| t.clips.iter().map(move |c| (t, c)))
        .collect::<Vec<_>>();
    let mut selected: BTreeSet<_> = seeds.iter().copied().collect();
    if seeds.is_empty() {
        selected.extend(all.iter().map(|(_, c)| c.id));
    }
    loop {
        let before = selected.len();
        for (track, clip) in &all {
            if !selected.contains(&clip.id) {
                continue;
            }
            for (other_track, other) in &all {
                let linked = clip.link.is_some() && clip.link == other.link;
                let same_track = track.id == other_track.id;
                // Whole selected and sync-locked tracks conservatively cover
                // neighbours and clips straddling an insert/delete boundary.
                let ripple = same_track || other_track.sync_lock;
                let caption = matches!(other.content, kinewright_core::ClipContent::Title(_));
                let synced = document.catalog.sync_groups.iter().any(|group| {
                    group.members.iter().any(|m| m.asset == clip.asset)
                        && group.members.iter().any(|m| m.asset == other.asset)
                });
                let routing = document.audio_mix.buses.iter().any(|bus| {
                    bus.tracks.contains(&track.id) && bus.tracks.contains(&other_track.id)
                });
                if linked || ripple || caption || synced || routing {
                    selected.insert(other.id);
                }
            }
        }
        if selected.len() == before {
            break;
        }
    }
    let mut rows = vec![
        json!({"timebase":document.fps,"duration":document.duration,"tracks":document.tracks.iter().map(|t|json!({"id":t.id,"role":t.kind,"sync_lock":t.sync_lock})).collect::<Vec<_>>(),"routing":document.audio_mix,"sync_groups":document.catalog.sync_groups,"markers":document.markers}),
    ];
    rows.extend(all.into_iter().filter(|(_,c)| selected.contains(&c.id)).map(|(t,c)| json!({
        "track":t.id,"track_role":t.kind,"sync_lock":t.sync_lock,"clip":c,
        "read_only_dependency":!seeds.contains(&c.id),"asset": document.asset(c.asset).map(|a| json!({"id":a.id,"duration":a.duration,"fps":a.fps,"kind":a.kind,"fingerprint":a.source_fingerprint}))
    })));
    rows
}
fn page(
    rows: &[Value],
    binding: &Value,
    cursor: Option<&str>,
    limit: usize,
) -> Result<Value, HostError> {
    // Fragment oversized logical rows independently of the page budget, so an
    // overview cursor continues into the larger expansion page without loss.
    let mut fragments = Vec::new();
    for (index, row) in rows.iter().enumerate() {
        let bytes = serde_json::to_vec(row)?;
        if bytes.len() <= 3072 {
            fragments.push(row.clone());
        } else {
            let hash = kinewright_media::sha256_bytes(&bytes);
            for chunk in bytes_rows(&bytes) {
                fragments.push(json!({"kind":"json_fragment","row":index,"sha256":hash,"byte_length":bytes.len(),"chunk":chunk}));
            }
        }
    }
    let rows = &fragments;
    // History metadata can advance without changing the result's source head.
    // A cursor binds both the authority and the complete data it paginates.
    let identity = kinewright_media::sha256_bytes(&serde_json::to_vec(&(binding, rows))?);
    let offset = if let Some(cursor) = cursor {
        let (hash, offset) = cursor
            .split_once(':')
            .ok_or_else(|| refused("invalid evidence cursor"))?;
        if hash != identity {
            return Err(refused("stale or unrelated evidence cursor"));
        }
        offset
            .parse::<usize>()
            .map_err(|_| refused("invalid evidence cursor"))?
    } else {
        0
    };
    if offset > rows.len() {
        return Err(refused("invalid evidence cursor"));
    }
    let mut items = Vec::new();
    let mut end = offset;
    let mut result;
    loop {
        let next = (end < rows.len()).then(|| format!("{identity}:{end}"));
        result = json!({"binding":binding,"items":items,"omitted":end<rows.len(),"next_cursor":next,"total_items":rows.len(),"expansion_limit":EXPANSIONS});
        if serde_json::to_vec(&result)?.len() > limit {
            return Err(capacity());
        }
        if end == rows.len() {
            break;
        }
        let mut trial = result.clone();
        trial["items"]
            .as_array_mut()
            .unwrap()
            .push(rows[end].clone());
        if serde_json::to_vec(&trial)?.len() + 128 > limit {
            if items.is_empty() {
                return Err(capacity());
            }
            break;
        }
        items.push(rows[end].clone());
        end += 1;
    }
    Ok(result)
}
fn bytes_rows(bytes: &[u8]) -> Vec<Value> {
    if let Ok(text) = std::str::from_utf8(bytes) {
        let mut rows = Vec::new();
        let mut start = 0;
        while start < text.len() {
            let mut end = (start + 1024).min(text.len());
            while !text.is_char_boundary(end) {
                end -= 1;
            }
            rows.push(json!({"byte_offset":start,"encoding":"utf8","text":&text[start..end]}));
            start = end;
        }
        rows
    } else {
        use base64::Engine as _;
        bytes.chunks(1024).enumerate().map(|(index,chunk)|json!({"byte_offset":index*1024,"encoding":"base64","bytes":base64::engine::general_purpose::STANDARD.encode(chunk)})).collect()
    }
}

pub(super) struct PreparedMediaRead {
    lease: kinewright_project::owned_inputs::OwnedInputLease,
    micros: i64,
    audio: bool,
    at: i64,
    reference: ObjectRef,
    closure: Vec<Value>,
    binding: Value,
    #[cfg(test)]
    gate: Option<Arc<(std::sync::Barrier, std::sync::Barrier)>>,
}
pub(super) struct RenderedMediaRead {
    binding: Value,
    rows: Vec<Value>,
}

// FFmpeg's linesize includes padding. Never expose padding or inactive samples.
fn visible_prefix(
    data: &[u8],
    stride: usize,
    row_bytes: usize,
    height: usize,
    limit: usize,
) -> Result<Vec<u8>, HostError> {
    if stride == 0
        || row_bytes == 0
        || row_bytes > stride
        || height
            .checked_mul(stride)
            .is_none_or(|size| size > data.len())
    {
        return Err(refused("invalid visible frame bounds"));
    }
    let mut output = Vec::new();
    for row in data.chunks(stride).take(height) {
        let count = row_bytes.min(limit.saturating_sub(output.len()));
        output.extend_from_slice(&row[..count]);
        if output.len() == limit {
            break;
        }
    }
    Ok(output)
}
impl PreparedMediaRead {
    // Consumes decoder pins before the caller reacquires the host owner lock.
    pub(super) fn render(self) -> Result<RenderedMediaRead, HostError> {
        #[cfg(test)]
        if let Some(gate) = &self.gate {
            gate.0.wait();
            gate.1.wait();
        }
        let source = self.lease.certified_source()?;
        let cancel = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let mut value = if self.audio {
            let mut decoder = source.open_audio(cancel)?;
            decoder
                .seek(self.micros)
                .map_err(|_| refused("owned audio unavailable"))?;
            let frame = decoder
                .next_frame()
                .map_err(|_| refused("owned audio unavailable"))?
                .ok_or_else(|| refused("owned audio unavailable"))?;
            let channels = usize::from(frame.channels());
            let bytes = frame.format().bytes();
            let frame_bytes = channels
                .checked_mul(bytes)
                .filter(|n| *n > 0 && *n <= 2048)
                .ok_or_else(|| refused("unsupported audio sample format"))?;
            let samples = frame.samples().min(2048 / frame_bytes);
            let planar = frame.format().is_planar();
            if channels > 8 || frame.planes() == 0 || (planar && frame.planes() < channels) {
                return Err(refused("unsupported audio sample layout"));
            }
            // Audio linesize[0] describes every planar channel; the wrapper's
            // raw data(n) accessor instead reads linesize[n]. Typed planes
            // expose exactly the initialized sample count, without padding.
            let planes = if planar {
                let mut planes = Vec::new();
                for channel in 0..channels {
                    macro_rules! numeric_plane {
                        ($sample:ty) => {
                            frame
                                .plane::<$sample>(channel)
                                .iter()
                                .take(samples)
                                .flat_map(|sample| sample.to_ne_bytes())
                                .collect::<Vec<_>>()
                        };
                    }
                    planes.push(match frame.format().packed().name() {
                        "u8" => frame
                            .plane::<u8>(channel)
                            .iter()
                            .take(samples)
                            .copied()
                            .collect(),
                        "s16" => numeric_plane!(i16),
                        "s32" => numeric_plane!(i32),
                        "flt" => numeric_plane!(f32),
                        "dbl" => numeric_plane!(f64),
                        _ => return Err(refused("unsupported planar audio sample format")),
                    });
                }
                planes
            } else {
                vec![
                    frame
                        .data(0)
                        .get(..samples * frame_bytes)
                        .ok_or_else(|| refused("invalid audio sample bounds"))?
                        .to_vec(),
                ]
            };
            let mut data = Vec::new();
            for sample in 0..samples {
                for channel in 0..channels {
                    let plane = if planar { channel } else { 0 };
                    let offset = if planar {
                        sample * bytes
                    } else {
                        sample * frame_bytes + channel * bytes
                    };
                    data.extend_from_slice(
                        planes[plane]
                            .get(offset..offset + bytes)
                            .ok_or_else(|| refused("invalid audio sample bounds"))?,
                    );
                }
            }
            json!({"kind":"owned_audio_sample","format":format!("{:?}",frame.format().packed()),"packing":"interleaved","samples":samples,"channels":channels,"rate":frame.rate(),"pts":frame.pts(),"data":data})
        } else {
            let mut decoder = source.open_video(cancel)?;
            decoder
                .seek(self.micros)
                .map_err(|_| refused("owned frame unavailable"))?;
            let frame = decoder
                .next_frame()
                .map_err(|_| refused("owned frame unavailable"))?
                .ok_or_else(|| refused("owned frame unavailable"))?;
            let descriptor = frame
                .format()
                .descriptor()
                .ok_or_else(|| refused("unsupported frame format"))?;
            let name = descriptor.name();
            let bytes = match name {
                "yuv420p" | "yuv422p" | "yuv444p" | "yuvj420p" | "yuvj422p" | "yuvj444p"
                | "gray" | "nv12" | "nv21" => 1,
                "rgb24" | "bgr24" => 3,
                "rgba" | "bgra" | "argb" | "abgr" => 4,
                _ => return Err(refused("unsupported visible frame format")),
            };
            let row_bytes = frame.width() as usize * bytes;
            let data = visible_prefix(
                frame.data(0),
                frame.stride(0),
                row_bytes,
                frame.height() as usize,
                2048,
            )?;
            json!({"kind":"owned_frame_sample","format":name,"width":frame.width(),"height":frame.height(),"visible_row_bytes":row_bytes,"pts":frame.pts(),"plane":0,"omitted_planes":true,"data":data})
        };
        value["partial_coverage"] = json!(true);
        value["requested_source_frame"] = json!(self.at);
        value["source_alignment"] =
            json!("unobserved; seek result may precede the requested position");
        value["source"] = serde_json::to_value(&self.reference)?;
        // Dependencies remain independently pageable, even for a large graph.
        let mut rows = vec![value];
        rows.extend(self.closure);
        Ok(RenderedMediaRead {
            binding: self.binding,
            rows,
        })
    }
}

impl HostCommandOwner {
    pub(super) fn context_snapshot(
        &self,
        run: &RunId,
        task: &TaskId,
    ) -> Result<(TimelineRevision, Arc<Document>), HostError> {
        let core = self
            .hooks
            .runs
            .get(run)
            .and_then(|r| r.tasks.get(task))
            .ok_or_else(|| refused("task Core unavailable"))?
            .core();
        match core
            .request(Command::Query(Query::Snapshot))
            .map_err(|_| refused("task snapshot unavailable"))?
        {
            Event::QueryResult(QueryResult::Snapshot { revision, document }) => {
                Ok((revision, document))
            }
            _ => Err(refused("task snapshot unavailable")),
        }
    }
    pub(super) fn context_notes(&self, task: &TaskRecord) -> Result<Vec<Value>, HostError> {
        use kinewright_project::studio_records::{FeedbackBatch, FeedbackRevision};
        let store = self.owner()?.store()?;
        let plan = store.read_evidence(&task.brief.plan)?;
        let value: Value = serde_json::from_slice(&plan).unwrap_or(Value::Null);
        if value["kind"] != "revise_cut_plan" {
            return Ok(vec![]);
        }
        let reference = serde_json::from_value(value["batch_ref"].clone())?;
        let batch = store.read_record::<FeedbackBatch>(&reference)?;
        batch.revision_refs.iter().map(|reference| {
            let revision = store.read_record::<FeedbackRevision>(reference)?;
            Ok(json!({"note_id":revision.comment_id,"anchor":revision.anchor,"text_ref":revision.original_text_ref,"proof_ref":revision.frame_proof_ref,"transcript_ref":revision.transcript_revision,"voice":revision.voice_ref,"detail_ref":reference}))
        }).collect()
    }
    fn timeline_context_rows(
        &self,
        task: &TaskRecord,
        document: &Document,
        seeds: &[ClipId],
    ) -> Result<Vec<Value>, HostError> {
        let mut rows = clip_rows(document, seeds);
        rows.push(json!({"kind":"task_evidence_catalog","note_anchors":self.context_notes(task)?,"brief_refs":[task.brief.plan.clone(),task.brief.script.clone(),task.brief.assignment.clone()],"owned_inputs":task.brief.inputs,"omitted_evidence":["full transcript","full media","unobserved proof"],"read_route":"read_task_evidence"}));
        Ok(rows)
    }
    pub(super) fn context_overview(
        &self,
        run: &RunId,
        task: &TaskRecord,
        revision: TimelineRevision,
        document: &Document,
    ) -> Result<Value, HostError> {
        let notes = self.context_notes(task)?;
        let seeds = task
            .brief
            .scope
            .objects
            .iter()
            .filter_map(|s| s.strip_prefix("clip:")?.parse().ok().map(ClipId))
            .collect::<Vec<_>>();
        let rows = self.timeline_context_rows(task, document, &seeds)?;
        let context = page(&rows,&json!({"run":run,"task":task.id,"revision":revision,"document_sha256":kinewright_media::sha256_bytes(&serde_json::to_vec(document)?),"evidence":{"kind":"timeline","clips":seeds}}),None,OVERVIEW_BYTES/2).ok();
        let mut overview = json!({"revision":revision,"timebase":document.fps,"duration":document.duration,
            "tracks":document.tracks.iter().map(|t|json!({"id":t.id,"role":t.kind,"sync_lock":t.sync_lock})).collect::<Vec<_>>(),
            "caption_boundaries":document.tracks.iter().flat_map(|t|&t.clips).filter(|c|matches!(c.content,kinewright_core::ClipContent::Title(_))).map(|c|json!({"clip":c.id,"start":c.timeline_start,"local_span":c.source_range})).collect::<Vec<_>>(),
            "note_anchors":notes,"available_evidence":{"brief":[task.brief.plan.clone(),task.brief.script.clone(),task.brief.assignment.clone()],"owned_inputs":task.brief.inputs,"media":"owned samples via read_task_evidence; unavailable streams refuse","transcript":"only admitted references; unreported transcript is unavailable","preview_proof":"references only; unobserved proof is unknown"},
            "omitted_evidence":["full transcript","full media","full delta and history","unobserved proof"],"read_route":"read_task_evidence",
            "context":context});
        if overview["context"].is_null() || serde_json::to_vec(&overview)?.len() > OVERVIEW_BYTES {
            overview = json!({"revision":revision,"timebase":document.fps,"duration":document.duration,"track_count":document.tracks.len(),"note_count":notes.len(),"omitted":true,"omitted_fields":["tracks","relationships","caption_boundaries","note_anchors","evidence"],"read_route":"read_task_evidence","request":{"revision":revision,"evidence":{"kind":"timeline","clips":[]}}});
        }
        if overview["context"]["omitted"] == true {
            overview["next_read"] = json!({"revision":revision,"evidence":overview["context"]["binding"]["evidence"],"cursor":overview["context"]["next_cursor"]});
        }
        Ok(overview)
    }
    fn context_reference_allowed(
        &self,
        state: &RunProjection,
        task: &TaskRecord,
        reference: &ObjectRef,
    ) -> Result<bool, HostError> {
        if [
            &task.brief.plan,
            &task.brief.script,
            &task.brief.inputs,
            &task.brief.assignment,
        ]
        .contains(&reference)
            || task.brief.scope.asset_roots.contains(reference)
            || task.brief.scope.consent_refs.contains(reference)
        {
            return Ok(true);
        }
        for note in self.context_notes(task)? {
            for name in ["text_ref", "proof_ref", "transcript_ref"] {
                if note[name] == serde_json::to_value(reference)? {
                    return Ok(true);
                }
            }
        }
        for child in &state.tasks {
            if child.parent.as_ref() != Some(&task.id) {
                continue;
            }
            if let crate::orchestration::orchestration_records::TaskState::Terminal(result) =
                &child.state
            {
                let record = self.owner()?.store()?.read_run_record(result)?;
                if let OrchestrationPayload::Result(result) = &record.payload
                    && (result.evidence.contains(reference)
                        || result.artifacts.contains(reference)
                        || result.delta_ref.as_ref() == Some(reference)
                        || result.explanation == *reference
                        || result.usage_ref == *reference)
                {
                    return Ok(true);
                }
            }
        }
        Ok(false)
    }
    pub(super) fn expansion_count(&self, task: &TaskRecord) -> Result<usize, HostError> {
        let mut count = 0;
        for reference in &task.progress.decisions {
            if reference.record_kind != RecordKind::OwnedEvidence {
                continue;
            }
            let bytes = self.owner()?.store()?.read_evidence(reference)?;
            let value: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
            if value["kind"] == "task_evidence_expansion_v1" {
                count += 1;
            }
        }
        Ok(count)
    }
    fn record_expansion(
        &mut self,
        index: usize,
        state: &RunProjection,
        task: &TaskRecord,
        binding: &Value,
    ) -> Result<(), HostError> {
        if self.expansion_count(task)? >= EXPANSIONS {
            return Err(capacity());
        }
        let reference = self.owner_mut()?.admit_host_grant(&serde_json::to_vec(&json!({"kind":"task_evidence_expansion_v1","request_sha256":kinewright_media::sha256_bytes(&serde_json::to_vec(binding)?)}))?)?;
        let issued = self.issue_metadata(index, None)?;
        let scope = self.attachments[index]
            .producer_scope
            .as_ref()
            .ok_or_else(|| refused("task scope unavailable"))?;
        let mut envelope = OrchestrationEnvelope {
            id: issued.envelope.id,
            digest: serde_json::from_value(json!("0".repeat(64)))?,
            project: issued.envelope.project_id,
            expected_generation: self.owner()?.metadata_generation()?,
            authority: AuthorityBinding {
                grant_ref: issued.grant_ref,
                owner_epoch: issued.envelope.owner_epoch,
                endpoint: scope.endpoint.clone(),
                attempt: scope.attempt.clone(),
            },
            command: OrchestrationCommand::RecordContext {
                task: task.id.clone(),
                feedback: vec![],
                decisions: vec![reference],
            },
        };
        envelope.digest = envelope
            .computed_digest()
            .map_err(|_| refused("context digest"))?;
        let receipt = self.dispatch_orchestration(
            index,
            &super::orchestration::ScopedOrchestrationRequest {
                run: Some(state.run.clone()),
                envelope,
                consume_batch: None,
            },
        )?;
        if receipt.decision != crate::orchestration::orchestration_commit::CommandDecision::Applied
        {
            return Err(refused("evidence audit not committed"));
        }
        Ok(())
    }
    pub(super) fn read_context(
        &mut self,
        index: usize,
        state: &RunProjection,
        task: &TaskRecord,
        read: &EvidenceRead,
    ) -> Result<Value, HostError> {
        if self.expansion_count(task)? >= EXPANSIONS {
            return Err(capacity());
        }
        let (revision, document) = self.context_snapshot(&state.run, &task.id)?;
        if revision != read.revision {
            return Err(refused("stale evidence revision"));
        }
        let binding = json!({"run":state.run,"task":task.id,"revision":revision,"document_sha256":kinewright_media::sha256_bytes(&serde_json::to_vec(&*document)?),"evidence":read.evidence});
        let rows = match &read.evidence {
            EvidenceKind::Timeline { clips } => {
                if clips.len() > 256
                    || clips.iter().any(|id| {
                        !document
                            .tracks
                            .iter()
                            .any(|t| t.clips.iter().any(|c| c.id == *id))
                    })
                {
                    return Err(refused("unknown timeline clip"));
                }
                self.timeline_context_rows(task, &document, clips)?
            }
            EvidenceKind::Audio { clip, source_frame }
            | EvidenceKind::Frame { clip, source_frame } => {
                let _ = (clip, source_frame);
                return Err(refused("media read must release owner before decoding"));
            }
            EvidenceKind::Reference { reference }
            | EvidenceKind::Transcript { reference, .. }
            | EvidenceKind::Preview { reference }
            | EvidenceKind::Proof { reference } => {
                if !self.context_reference_allowed(state, task, reference)? {
                    return Err(refused("read reference outside task scope"));
                }
                let value = self.read_task_reference(reference)?;
                let classification = match &read.evidence {
                    EvidenceKind::Preview { .. } if value["kind"] != "task_preview_v1" => {
                        return Err(refused("reference is not a classified preview"));
                    }
                    EvidenceKind::Proof { .. }
                        if value["kind"] != "task_proof_v1"
                            && !self.context_notes(task)?.iter().any(|note| {
                                note["proof_ref"]
                                    == serde_json::to_value(reference).unwrap_or(Value::Null)
                            }) =>
                    {
                        return Err(refused("reference is not a classified proof"));
                    }
                    EvidenceKind::Preview { .. } => "preview",
                    EvidenceKind::Proof { .. } => "proof",
                    EvidenceKind::Transcript { .. } => "transcript",
                    _ => "reference",
                };
                if matches!(
                    read.evidence,
                    EvidenceKind::Preview { .. } | EvidenceKind::Proof { .. }
                ) && value
                    .get("revision")
                    .is_some_and(|stored| *stored != json!(revision))
                {
                    return Err(refused("stale evidence proof revision"));
                }
                let bytes = serde_json::to_vec(&value)?;
                if let EvidenceKind::Transcript { start, end, .. } = &read.evidence {
                    let words = if let Some(words) = value.get("words").and_then(Value::as_array) {
                        words.clone()
                    } else if let Some(text) = value.as_str() {
                        // Saved voice transcript revisions are UTF-8 text, not
                        // time-aligned words. Preserve that missing alignment.
                        text.split_whitespace().map(|word| json!(word)).collect()
                    } else {
                        return Err(refused("transcript words unavailable"));
                    };
                    if start >= end || *end > words.len() {
                        return Err(refused("transcript span invalid"));
                    }
                    let first = start.saturating_sub(2);
                    let last = end.saturating_add(2).min(words.len());
                    let clip = value["clip"].as_u64().map(ClipId);
                    let mut rows = vec![
                        json!({"kind":"transcript_span","reference":reference,"requested_words":[start,end],"neighbours":2,"source_alignment":if clip.is_some(){"clip-bound"}else{"unobserved"},"omitted_dependencies":clip.is_none(),"dependency_read":{"revision":revision,"evidence":{"kind":"timeline","clips":clip.into_iter().collect::<Vec<_>>()}}}),
                    ];
                    rows.extend(words[first..last].iter().enumerate().map(|(i,w)|json!({"word_index":first+i,"word":w,"neighbour":first+i<*start||first+i>=*end})));
                    if let Some(clip) = clip {
                        if !document
                            .tracks
                            .iter()
                            .any(|t| t.clips.iter().any(|c| c.id == clip))
                        {
                            return Err(refused("transcript source clip unavailable"));
                        }
                        rows.extend(clip_rows(&document, &[clip]));
                    }
                    rows
                } else if bytes.len() <= 4096 {
                    vec![
                        json!({"classification":classification,"reference":reference,"data":value,"proof_verification":"unobserved"}),
                    ]
                } else {
                    bytes_rows(&bytes)
                }
            }
        };
        let value = page(&rows, &binding, read.cursor.as_deref(), PAGE_BYTES - 4096)?;
        self.record_expansion(index, state, task, &binding)?;
        Ok(value)
    }
    pub(super) fn prepare_media_context(
        &mut self,
        state: &RunProjection,
        task: &TaskRecord,
        read: &EvidenceRead,
    ) -> Result<PreparedMediaRead, HostError> {
        use kinewright_project::owned_inputs::OwnedInputRef;
        if self.expansion_count(task)? >= EXPANSIONS {
            return Err(capacity());
        }
        let (revision, document) = self.context_snapshot(&state.run, &task.id)?;
        if revision != read.revision {
            return Err(refused("stale evidence revision"));
        }
        let (clip_id, at, audio) = match read.evidence {
            EvidenceKind::Audio { clip, source_frame } => (clip, source_frame, true),
            EvidenceKind::Frame { clip, source_frame } => (clip, source_frame, false),
            _ => return Err(refused("media read required")),
        };
        let clip = document
            .tracks
            .iter()
            .flat_map(|t| &t.clips)
            .find(|c| c.id == clip_id)
            .ok_or_else(|| refused("media clip unavailable"))?;
        let asset = document
            .asset(clip.asset)
            .ok_or_else(|| refused("media asset unavailable"))?;
        if at < 0 || at >= asset.duration.0 {
            return Err(refused("media sample outside source"));
        }
        let store = self.owner()?.store()?;
        let manifest = store.read_input_manifest(&task.brief.inputs)?;
        let entry = manifest
            .entries
            .iter()
            .find(|e| e.matches_media_asset(asset))
            .ok_or_else(|| refused("media is not owned by task inputs"))?;
        let lease = store.lease_input(&OwnedInputRef {
            manifest_ref: task.brief.inputs.clone(),
            entry: entry.clone(),
        })?;
        let micros = i64::try_from(
            i128::from(at) * 1_000_000 * i128::from(asset.fps.denominator())
                / i128::from(asset.fps.numerator()),
        )
        .map_err(|_| refused("media time overflow"))?;
        let binding = json!({"run":state.run,"task":task.id,"revision":revision,"document_sha256":kinewright_media::sha256_bytes(&serde_json::to_vec(&*document)?),"evidence":read.evidence});
        Ok(PreparedMediaRead {
            lease,
            micros,
            audio,
            at,
            reference: entry.object.clone(),
            closure: clip_rows(&document, &[clip_id]),
            binding,
            #[cfg(test)]
            gate: self.hooks.context_media_gate.take(),
        })
    }
    pub(super) fn finish_media_context(
        &mut self,
        index: usize,
        state: &RunProjection,
        task: &TaskRecord,
        read: &EvidenceRead,
        rendered: &RenderedMediaRead,
    ) -> Result<Value, HostError> {
        let (revision, document) = self.context_snapshot(&state.run, &task.id)?;
        let binding = json!({"run":state.run,"task":task.id,"revision":revision,"document_sha256":kinewright_media::sha256_bytes(&serde_json::to_vec(&*document)?),"evidence":read.evidence});
        if binding != rendered.binding {
            return Err(refused("stale evidence revision after decode"));
        }
        let value = page(
            &rendered.rows,
            &binding,
            read.cursor.as_deref(),
            PAGE_BYTES - 4096,
        )?;
        self.record_expansion(index, state, task, &binding)?;
        Ok(value)
    }
    pub(super) fn read_legacy_context(
        &mut self,
        index: usize,
        state: &RunProjection,
        task: &TaskRecord,
        reference: &ObjectRef,
    ) -> Result<Value, HostError> {
        let value = self.read_task_reference(reference)?;
        if serde_json::to_vec(&value)?.len() > PAGE_BYTES - 4096 {
            return Err(capacity());
        }
        self.record_expansion(
            index,
            state,
            task,
            &json!({"run":state.run,"task":task.id,"reference":reference}),
        )?;
        Ok(value)
    }
    pub(super) fn read_child_context(
        &mut self,
        index: usize,
        state: &RunProjection,
        parent: &TaskRecord,
        read: &ChildRead,
    ) -> Result<Value, HostError> {
        if self.expansion_count(parent)? >= EXPANSIONS {
            return Err(capacity());
        }
        let child = state
            .tasks
            .iter()
            .find(|c| c.id == read.child && c.parent.as_ref() == Some(&parent.id))
            .ok_or_else(|| refused("result is not an authenticated child"))?;
        if read.run != state.run
            || !matches!(&child.state,crate::orchestration::orchestration_records::TaskState::Terminal(reference) if *reference==read.reference)
        {
            return Err(refused("child result binding changed"));
        }
        let record = self.owner()?.store()?.read_run_record(&read.reference)?;
        let OrchestrationPayload::Result(result) = &record.payload else {
            return Err(refused("child result record required"));
        };
        if result.head != read.source {
            return Err(refused("stale child source revision"));
        }
        let binding = json!({"run":state.run,"parent":parent.id,"child":child.id,"reference":read.reference,"source":read.source});
        let history = self.saved_task_history(state, &child.id)?;
        let mut rows = vec![
            json!({"result":result,"disclosure":self.result_disclosure(&state.run,child,result,&read.reference)?}),
        ];
        for (kind, reference) in [
            ("explanation", Some(&result.explanation)),
            ("delta", result.delta_ref.as_ref()),
            ("usage", Some(&result.usage_ref)),
        ] {
            if let Some(reference) = reference {
                for chunk in bytes_rows(&serde_json::to_vec(&self.read_task_reference(reference)?)?)
                {
                    rows.push(json!({"kind":kind,"reference":reference,"chunk":chunk}));
                }
            }
        }
        if let Some(history) = history {
            for chunk in bytes_rows(&serde_json::to_vec(&history)?) {
                rows.push(json!({"kind":"history","chunk":chunk}));
            }
        }
        let value = page(&rows, &binding, read.cursor.as_deref(), PAGE_BYTES - 4096)?;
        self.record_expansion(index, state, parent, &binding)?;
        Ok(value)
    }
    pub(super) fn result_disclosure(
        &self,
        run: &RunId,
        task: &TaskRecord,
        result: &TaskResult,
        reference: &ObjectRef,
    ) -> Result<Value, HostError> {
        let notes = self.context_notes(task)?;
        let bytes = self.owner()?.store()?.read_evidence(&result.explanation)?;
        let report: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
        let classified = report["kind"] == "task_result_report_v1"
            && report["partial_coverage"].is_boolean()
            && report["proof_status"]
                .as_str()
                .is_some_and(|value| matches!(value, "unknown" | "failed" | "reported_pass"))
            && [
                "satisfied_note_ids",
                "unmet_note_ids",
                "conflicts",
                "proof_failures",
                "changed_scope",
            ]
            .iter()
            .all(|key| {
                report[*key]
                    .as_array()
                    .is_some_and(|values| values.iter().all(Value::is_string))
            });
        let field = |key: &str| {
            if classified {
                report.get(key).cloned().unwrap_or(Value::Null)
            } else {
                Value::Null
            }
        };
        Ok(
            json!({"outcome":result.outcome,"partial_coverage":!classified || report["partial_coverage"]!=false,"coverage_provenance":"reported; unverified facts require expansion",
            "satisfied_note_ids":field("satisfied_note_ids"),"unmet_note_ids":field("unmet_note_ids"),"expected_note_ids":notes.iter().map(|n|n["note_id"].clone()).collect::<Vec<_>>(),
            "conflicts":field("conflicts"),"proof_status":if classified{field("proof_status")}else{json!("unknown")},"proof_failures":field("proof_failures"),
            "changed_scope":field("changed_scope"),"omitted_changes":true,"revision_kind":"checkpoint_head","revision":result.head.as_ref().map(|h|h.head.clone()),"base":result.base,"head":result.head,"evidence_refs":result.evidence,"proof_refs":result.artifacts,
            "detail":{"tool":"read_child_result","run":run,"child":task.id,"reference":reference,"source":result.head},"omitted_fields":["full delta","full history","unreported coverage and proof"]}),
        )
    }
}

#[cfg(target_os = "linux")]
impl HostCommandOwner {
    pub(super) fn context_result_wake(
        &self,
        state: &RunProjection,
        parent: &TaskId,
        references: &[&ObjectRef],
    ) -> Result<Vec<u8>, HostError> {
        let mut summaries = Vec::new();
        for reference in references {
            let child=state.tasks.iter().find(|t|t.parent.as_ref()==Some(parent) && matches!(&t.state,crate::orchestration::orchestration_records::TaskState::Terminal(r) if r==*reference)).ok_or_else(||refused("wake result is not an authenticated child"))?;
            let record = self.owner()?.store()?.read_run_record(reference)?;
            let OrchestrationPayload::Result(result) = &record.payload else {
                return Err(refused("wake result record required"));
            };
            let mut disclosure = self.result_disclosure(&state.run, child, result, reference)?;
            disclosure["detail"]["run"] = serde_json::to_value(&state.run)?;
            summaries.push(json!({"disclosure":disclosure}));
        }
        let full = self
            .result_wake_payload(references)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok());
        let detail_status = if full.is_some() {
            "available"
        } else {
            "expansion_unavailable"
        };
        if full.is_none() {
            for summary in &mut summaries {
                summary["disclosure"]["partial_coverage"] = json!(true);
            }
        }
        let detailed = json!({"mode":"full","results":summaries,"full_records":full,"omitted_details":false,"detail_status":detail_status});
        if !detailed["full_records"].is_null()
            && serde_json::to_vec(&detailed)?.len() <= PAGE_BYTES / 2
        {
            return Ok(serde_json::to_vec(&detailed)?);
        }
        if serde_json::to_vec(&summaries)?.len() > PAGE_BYTES / 2 - 512 {
            for summary in &mut summaries {
                let disclosure = &mut summary["disclosure"];
                let mut omitted = Vec::new();
                let mut counts = serde_json::Map::new();
                for field in [
                    "satisfied_note_ids",
                    "unmet_note_ids",
                    "expected_note_ids",
                    "conflicts",
                    "proof_failures",
                    "changed_scope",
                    "evidence_refs",
                    "proof_refs",
                ] {
                    if let Some(values) = disclosure[field].as_array() {
                        counts.insert(field.into(), json!(values.len()));
                        let trimmed = values
                            .iter()
                            .take(4)
                            .map(|value| {
                                if let Some(text) = value.as_str() {
                                    json!(text.chars().take(128).collect::<String>())
                                } else {
                                    value.clone()
                                }
                            })
                            .collect::<Vec<_>>();
                        if trimmed != *values {
                            omitted.push(field);
                        }
                        disclosure[field] = json!(trimmed);
                    }
                }
                if !omitted.is_empty() {
                    disclosure["omitted_disclosure_fields"] = json!(omitted);
                    disclosure["disclosure_counts"] = json!(counts);
                    disclosure["partial_coverage"] = json!(true);
                    disclosure["decision"] = json!("expand_before_accepting");
                }
            }
        }
        let summary = serde_json::to_vec(
            &json!({"mode":"summary","results":summaries,"omitted_details":true,"detail_status":detail_status,"detail_route":"read_child_result"}),
        )?;
        if summary.len() > PAGE_BYTES / 2 {
            return Err(capacity());
        }
        Ok(summary)
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn te14_visible_frame_sample_skips_padding() {
        let sample =
            super::visible_prefix(&[1, 2, 238, 238, 3, 4, 238, 238], 4, 2, 2, 2048).unwrap();
        assert_eq!(sample, vec![1, 2, 3, 4], "TE14_NO_FRAME_PADDING");
    }
    #[test]
    fn te14_large_rows_round_trip_and_changed_data_refuses_cursor() {
        use serde_json::json;
        let rows = vec![json!({"note":"é\"".repeat(12_000),"unmet":true})];
        let binding = json!({"revision":1});
        let mut cursor = None;
        let mut chunks = Vec::new();
        let mut first_cursor = None;
        loop {
            let page = super::page(&rows, &binding, cursor.as_deref(), 4096)
                .expect("TE14_LARGE_ROW_EXACT_EXPANSION");
            assert!(
                serde_json::to_vec(&page).unwrap().len() <= 4096,
                "TE14_LARGE_ROW_PAGE_BOUND"
            );
            for row in page["items"].as_array().unwrap() {
                chunks.push(row["chunk"]["text"].as_str().unwrap().to_owned());
            }
            cursor = page["next_cursor"].as_str().map(str::to_owned);
            if first_cursor.is_none() {
                first_cursor = cursor.clone();
            }
            if cursor.is_none() {
                break;
            }
        }
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&chunks.concat()).unwrap(),
            rows[0],
            "TE14_LARGE_ROW_EXACT_EXPANSION"
        );
        let mut changed = rows.clone();
        changed[0]["unmet"] = json!(false);
        assert!(
            super::page(&changed, &binding, first_cursor.as_deref(), 4096).is_err(),
            "TE14_CURSOR_BINDS_DATA"
        );
    }
    #[test]
    fn te14_role_prefix_goldens_are_fixed_ordered_bytes() {
        use crate::orchestration::orchestration_records::Role;
        let hashes = [
            Role::Director,
            Role::Editor,
            Role::Motion,
            Role::Audio,
            Role::Colour,
            Role::Reviewer,
        ]
        .map(|role| {
            let prefix = super::application_prefix(role).unwrap();
            assert!(
                prefix.starts_with(b"{\"instructions\":"),
                "TE14_FIXED_TEXT_FIRST"
            );
            assert!(prefix.ends_with(b",\"data\":"), "TE14_VARIABLE_DATA_LAST");
            assert!(
                !String::from_utf8(prefix.clone())
                    .unwrap()
                    .contains("provider_request_order"),
                "TE14_NO_UNOBSERVED_CACHE_CLAIM"
            );
            let hash = kinewright_media::sha256_bytes(&prefix);
            println!("TE14_PREFIX {role:?} {} {hash}", prefix.len());
            hash
        });
        if let Some(path) = std::env::var_os("KINEWRIGHT_TE14_CAPTURE") {
            std::fs::write(
                std::path::Path::new(&path).join("te14-prefixes.json"),
                serde_json::to_vec(&hashes).unwrap(),
            )
            .unwrap();
        } else {
            let expected: Vec<String> =
                serde_json::from_str(include_str!("../../tests/fixtures/te14-prefixes.json"))
                    .unwrap();
            assert_eq!(
                hashes.as_slice(),
                expected.as_slice(),
                "TE14_PREFIX_BYTES_STABLE"
            );
        }
    }
}
