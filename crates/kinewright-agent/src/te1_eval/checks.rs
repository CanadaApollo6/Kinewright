//! Behavior checks over the durable output, production preview and exported PCM.
#[cfg(target_os = "linux")]
use super::{Call, Trace, error, fixtures};
#[cfg(target_os = "linux")]
use kinewright_core::{
    Core, Document, Export, ExportCancellation, ExportSettings, Playback, TimeCode,
};
use serde::{Deserialize, Serialize};
#[cfg(target_os = "linux")]
use serde_json::Value;
#[cfg(target_os = "linux")]
use std::{
    io,
    path::Path,
    sync::Arc,
    time::{Duration, Instant},
};
#[derive(Debug, Serialize, Deserialize)]
#[allow(
    clippy::struct_excessive_bools,
    reason = "Independent oracle observations, not mutable lifecycle flags"
)]
pub struct Correctness {
    pub document: bool,
    pub document_difference: Option<String>,
    pub history_reopened: bool,
    pub task_undo_redo: bool,
    pub unrelated_clips: bool,
    pub trim_alignment: bool,
    pub observed_requirements: bool,
    pub rendered_frames: bool,
    pub audio_samples: bool,
    pub frame_checks: usize,
    pub pcm_samples: usize,
}
impl Correctness {
    #[must_use]
    pub fn passed(&self) -> bool {
        self.document
            && self.history_reopened
            && self.task_undo_redo
            && self.unrelated_clips
            && self.trim_alignment
            && self.observed_requirements
            && self.rendered_frames
            && self.audio_samples
    }
}
#[cfg(target_os = "linux")]
pub(super) fn verify(
    folder: &Path,
    case: u8,
    data: &Value,
    actual: &Document,
    history: &Value,
    trace: &Trace,
) -> io::Result<Correctness> {
    let mut oracle = kinewright_project::load_document(
        &fixtures().join(format!("case-{case}-oracle.kinewright")),
    )
    .map_err(error)?
    .0;
    let render_oracle = oracle.clone();
    for asset in &mut oracle.media_pool {
        if asset.id.0 == 1 {
            asset.kind = kinewright_core::MediaKind::Video;
        }
        if asset.id.0 == 2 {
            asset.kind = kinewright_core::MediaKind::AudioVideo;
            asset.resolution = Some((16, 16));
            asset.color_description = kinewright_core::ColorDescription {
                primaries: kinewright_core::ColorPrimaries::Bt709,
                transfer: kinewright_core::ColorTransfer::Bt709,
                matrix: kinewright_core::ColorMatrix::Bt709,
                range: kinewright_core::ColorRange::Limited,
                white_point: kinewright_core::ColorWhitePoint::D65,
                bit_depth: kinewright_core::ColorBitDepth::Eight,
                confidence_basis_points: 10000,
                provenance: kinewright_core::ColorProvenance::UserOverride,
            };
        }
        let path = super::media_source(asset.id.0);
        asset.path = path.file_name().unwrap().into();
        asset.source_fingerprint = kinewright_core::MediaSourceFingerprint {
            content_sha256: Some(kinewright_media::sha256_file(&path).map_err(error)?),
            byte_len: Some(std::fs::metadata(path)?.len()),
        };
    }
    let original = kinewright_project::load_document(&fixtures().join("talking-head.kinewright"))
        .map_err(error)?
        .0;
    let history: kinewright_core::HistorySnapshot =
        serde_json::from_value(history.clone()).map_err(error)?;
    let (history_reopened, task_undo_redo) = histories(actual, &history, data)?;
    let clip = |document: &Document, id: u64| {
        document
            .tracks
            .iter()
            .flat_map(|t| &t.clips)
            .find(|c| c.id.0 == id)
            .cloned()
    };
    let unrelated_clips = data["unchanged_clips"]
        .as_array()
        .ok_or_else(|| error("unchanged clip oracle missing"))?
        .iter()
        .all(|id| clip(actual, id.as_u64().unwrap()) == clip(&original, id.as_u64().unwrap()));
    let trim_alignment = case != 6
        || data["trim_expectations"].as_array().is_some_and(|rows| {
            rows.iter().all(|r| {
                clip(actual, r["clip"].as_u64().unwrap()).is_some_and(|c| {
                    c.source_range.start.0 == r["source_start"]
                        && c.source_range.end.0 == r["source_end"]
                        && c.timeline_start.0 == r["timeline_start"]
                        && c.timeline_start.0 + c.source_range.end.0 - c.source_range.start.0
                            == r["timeline_end"]
                })
            })
        }) && clip(actual, 8)
            .is_some_and(|c| c.timeline_start.0 == 52 && c.source_range.end.0 == 38);
    let observed_requirements = observed(case, trace);
    let (rendered_frames, frame_checks, audio_samples, pcm_samples) =
        media(folder, actual, &render_oracle)?;
    let a = serde_json::to_value(actual)?;
    let b = serde_json::to_value(&oracle)?;
    let document_difference = super::pin::first_difference(&b, &a).map(|path| {
        format!(
            "{path}: expected {} actual {}",
            b.pointer(&path).unwrap_or(&Value::Null),
            a.pointer(&path).unwrap_or(&Value::Null)
        )
    });
    Ok(Correctness {
        document_difference,
        document: actual == &oracle,
        history_reopened,
        task_undo_redo,
        unrelated_clips,
        trim_alignment,
        observed_requirements,
        rendered_frames,
        audio_samples,
        frame_checks,
        pcm_samples,
    })
}
#[cfg(target_os = "linux")]
fn histories(
    actual: &Document,
    history: &kinewright_core::HistorySnapshot,
    _data: &Value,
) -> io::Result<(bool, bool)> {
    let reopened = Core::spawn((*history.baseline).clone()).map_err(error)?;
    reopened.restore_history(history.clone()).map_err(error)?;
    let history_reopened =
        reopened.export_history().map_err(error)? == *history && *history.document == *actual;
    for _ in &history.cursor.undo {
        reopened
            .request(kinewright_core::Command::Undo)
            .map_err(error)?;
    }
    let undone = reopened.export_history().map_err(error)?;
    let undo_ok = undone.document == history.baseline && undone.cursor.undo.is_empty();
    for _ in &history.cursor.undo {
        reopened
            .request(kinewright_core::Command::Redo)
            .map_err(error)?;
    }
    let redone = reopened.export_history().map_err(error)?;
    let task_undo_redo = !history.cursor.undo.is_empty()
        && undo_ok
        && redone.document == history.document
        && redone.cursor == history.cursor;
    Ok((history_reopened, task_undo_redo))
}
#[cfg(target_os = "linux")]
fn effective_merge_positions(calls: &[Call]) -> std::collections::BTreeSet<usize> {
    let mut merge_ids = std::collections::BTreeSet::new();
    calls
        .iter()
        .enumerate()
        .filter_map(|(position, call)| {
            if call.tool != "merge_task" || call.refused || call.fixture {
                return None;
            }
            let identity = [
                "/outcome/receipt/id",
                "/receipt/id",
                "/outcome/receipt/envelope/id",
                "/receipt/envelope/id",
            ]
            .iter()
            .find_map(|path| call.reply.pointer(path))
            .filter(|value| !value.is_null())?;
            merge_ids
                .insert(serde_json::to_string(&(call.task.clone(), identity)).ok()?)
                .then_some(position)
        })
        .collect::<std::collections::BTreeSet<_>>()
}
#[cfg(target_os = "linux")]
fn publication_identity(call: &Call) -> Option<&Value> {
    if call.refused || call.fixture {
        return None;
    }
    let receipt = match call.tool.as_str() {
        "finish_task" => call
            .reply
            .pointer("/completion_status/publication_receipt")
            .or_else(|| {
                call.reply
                    .pointer("/outcome/completion_status/publication_receipt")
            }),
        "publish_result" => call
            .reply
            .pointer("/outcome/receipt")
            .or_else(|| call.reply.get("receipt")),
        _ => None,
    }
    .filter(|value| !value.is_null())?;
    Some(
        receipt
            .get("id")
            .or_else(|| receipt.pointer("/envelope/id"))
            .unwrap_or(receipt),
    )
}
#[cfg(target_os = "linux")]
pub(crate) fn observed(case: u8, trace: &Trace) -> bool {
    let calls = &trace.calls;
    let effective_merges = effective_merge_positions(calls);
    let director = trace
        .sends
        .iter()
        .find(|send| send["brief"]["role"] == "director")
        .map(|send| &send["identity"]["task"]);
    match case {
        1 => true, // Exact document/history/media checks determine the effect.
        5 => {
            let anchor = calls.iter().enumerate().find_map(|(published, bad)| {
                publication_identity(bad)?;
                let explanation = if bad.tool == "publish_result" {
                    bad.arguments.pointer("/command/value/result/explanation")
                } else if bad.tool == "finish_task" {
                    let status = bad
                        .reply
                        .get("completion_status")
                        .or_else(|| bad.reply.pointer("/outcome/completion_status"));
                    status
                        .filter(|s| !s["publication_receipt"].is_null())
                        .and_then(|s| s.pointer("/result/explanation"))
                } else {
                    None
                };
                explanation
                    .filter(|v| !v.is_null())
                    .map(|explanation| (published, bad, explanation))
            });
            let Some((published, bad, explanation)) = anchor else {
                return false;
            };
            calls[published + 1..]
                .iter()
                .enumerate()
                .any(|(offset, read)| {
                    let read_position = published + offset + 1;
                    matches!(read.tool.as_str(), "read_child_result" | "read_task_result")
                        && Some(&read.task) == director
                        && ((read.tool == "read_child_result"
                            && read.arguments["child"] == bad.task)
                            || (read.tool == "read_task_result"
                                && read.arguments.get("reference") == Some(explanation)))
                        && !read.refused
                        && !read.fixture
                        && serde_json::to_string(&read.reply)
                            .is_ok_and(|s| s.contains("correct-homophone") && s.contains("failed"))
                        && effective_merges
                            .range(published + 1..read_position)
                            .next()
                            .is_none()
                        && calls[read_position + 1..]
                            .iter()
                            .enumerate()
                            .any(|(after, c)| {
                                effective_merges.contains(&(read_position + 1 + after))
                                    && c.tool == "merge_task"
                                    && !c.refused
                                    && !c.fixture
                                    && Some(&c.task) == director
                            })
                })
                && !calls.iter().any(|c| {
                    c.tool == "merge_task"
                        && !c.refused
                        && c.arguments["command"]["value"]["task"] == bad.task
                })
        }
        6 => {
            calls.iter().enumerate().any(|(stale, failed)| {
                failed.tool == "commit_edit_plan"
                    && failed.refused
                    && !failed.fixture
                    && calls[..stale].iter().any(|c| {
                        c.fixture
                            && !c.refused
                            && c.tool == "undo_task_edit"
                            && c.task == failed.task
                            && c.attempt == failed.attempt
                    })
                    && ["timeline", "transcript", "frame", "audio"]
                        .iter()
                        .all(|kind| {
                            calls[..stale]
                                .iter()
                                .any(|c| same_actor(c, failed) && evidence_read(c, kind))
                        })
                    && recovered(&calls[stale..], failed)
            }) && calls
                .iter()
                .any(|c| c.tool == "undo_task_edit" && !c.refused && c.fixture)
        }
        _ => false,
    }
}
#[cfg(target_os = "linux")]
fn same_actor(call: &Call, affected: &Call) -> bool {
    !call.fixture
        && call.task.is_string()
        && call.attempt.is_string()
        && call.task == affected.task
        && call.attempt == affected.attempt
}
#[cfg(target_os = "linux")]
fn recovered(calls: &[Call], affected: &Call) -> bool {
    let mut refreshed = false;
    let mut prepared = None;
    for call in calls {
        if !same_actor(call, affected) {
            continue;
        }
        let reply = call.reply.get("outcome").unwrap_or(&call.reply);
        if (matches!(call.tool.as_str(), "task_status" | "get_timeline_state")
            && !call.refused
            && reply.is_object())
            || call
                .reply
                .get("controls")
                .is_some_and(|v| v.is_object() && !v.as_object().unwrap().is_empty())
        {
            refreshed = true;
        }
        if call.tool == "prepare_edit_plan" && !call.refused && refreshed {
            prepared = reply
                .get("preview")
                .filter(|preview| {
                    preview["operation_count"].as_u64().is_some_and(|n| n > 0)
                        && !call.arguments["expected_revision"].is_null()
                        && preview["expected_revision"] == call.arguments["expected_revision"]
                })
                .and_then(|_| reply.get("plan_id"))
                .filter(|id| id.as_u64().is_some_and(|id| id > 0))
                .map(|id| (id.clone(), call.arguments["expected_revision"].clone()));
        }
        if call.tool == "commit_edit_plan"
            && !call.refused
            && prepared.as_ref().is_some_and(|(plan, revision)| {
                !revision.is_null()
                    && call.arguments["plan_id"] == *plan
                    && call.arguments["expected_revision"] == *revision
            })
        {
            return true;
        }
    }
    false
}
#[cfg(target_os = "linux")]
fn evidence_read(call: &Call, kind: &str) -> bool {
    if call.refused {
        return false;
    }
    if call.tool == "read_task_evidence" {
        return call.arguments["evidence"]["kind"] == kind;
    }
    let tool = if call.tool == "invoke_capability" {
        call.arguments["name"].as_str().unwrap_or("")
    } else {
        &call.tool
    };
    match kind {
        "timeline" => tool == "get_timeline_state",
        "transcript" => {
            tool == "read_task_result"
                && serde_json::to_string(&call.reply).is_ok_and(|s| s.contains("words"))
        }
        "frame" => tool == "get_frame_at",
        "audio" => tool == "get_audio_levels",
        _ => false,
    }
}
#[cfg(target_os = "linux")]
fn absolute(mut document: Document, oracle: bool) -> Document {
    for asset in &mut document.media_pool {
        if asset.path.is_relative() {
            asset.path = if oracle {
                fixtures().join(&asset.path)
            } else {
                super::media_source(asset.id.0)
            };
        }
    }
    document
}
#[cfg(target_os = "linux")]
fn media(
    folder: &Path,
    actual: &Document,
    oracle: &Document,
) -> io::Result<(bool, usize, bool, usize)> {
    let engine =
        kinewright_media::FfmpegMediaEngine::new_with_data_dir(folder.join("media-check-cache"))
            .map_err(error)?
            .into_legacy_fixture();
    let positions = [0, 20, 49, 50, 51, 52, 89, 90, 99];
    let mut frames = Vec::new();
    let mut audio = Vec::new();
    for (index, document) in [actual, oracle].into_iter().enumerate() {
        let document = Arc::new(absolute(document.clone(), index == 1));
        engine.set_document(document.clone());
        let receiver = engine.frames();
        let mut arm = Vec::new();
        for position in positions {
            engine.seek(TimeCode(position));
            while receiver.try_recv().is_ok() {}
            engine.request_frame(TimeCode(position));
            let stamp = engine.stamp();
            let deadline = Instant::now() + Duration::from_secs(30);
            let frame = loop {
                if let Ok(frame) = receiver.try_recv()
                    && frame.at == TimeCode(position)
                    && frame.stamp == stamp
                {
                    break frame;
                }
                if Instant::now() >= deadline {
                    return Err(error(format!(
                        "render deadline at {position}: {:?}",
                        engine.events().try_iter().collect::<Vec<_>>()
                    )));
                }
                std::thread::park_timeout(Duration::from_millis(2));
            };
            if (frame.texture.width, frame.texture.height) != (160, 90) {
                return Err(error("unexpected preview raster"));
            }
            arm.push(kinewright_media::sha256_bytes(&frame.texture.rgba));
        }
        frames.push(arm);
        let output = folder.join(format!("media-check-{index}.mp4"));
        let (progress, _) = crossbeam_channel::unbounded();
        engine
            .export_document(
                document.clone(),
                &output,
                ExportSettings {
                    fps: document.fps,
                    resolution: document.resolution,
                    delivery_color: kinewright_core::ColorContext::sdr_rec709().delivery,
                    video_codec: "libx264".into(),
                    audio_codec: "aac".into(),
                    video_bitrate: 500_000,
                    audio_bitrate: 128_000,
                    loudness_normalization: None,
                    cancellation: ExportCancellation::default(),
                },
                progress,
            )
            .map_err(error)?;
        audio.push(
            kinewright_media::decode_audio_range(
                &output,
                document.fps,
                TimeCode(0),
                TimeCode(100),
                48_000,
                1,
                &ExportCancellation::default(),
            )
            .map_err(error)?,
        );
    }
    let audio_samples = audio[0].len() == audio[1].len()
        && audio[0].iter().any(|s| s.abs() > 0.01)
        && audio[0]
            .iter()
            .zip(&audio[1])
            .all(|(a, b)| (a - b).abs() <= 1.0 / 32768.0);
    Ok((
        frames[0] == frames[1],
        positions.len(),
        audio_samples,
        audio[0].len(),
    ))
}

#[cfg(all(test, target_os = "linux"))]
#[path = "checks_tests.rs"]
mod tests;
