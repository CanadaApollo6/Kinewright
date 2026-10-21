//! Existing preview/proof/export/decoder paths. No decoder snapshot semantics.
#![allow(clippy::missing_errors_doc)]
use std::{
    sync::Arc,
    time::{Duration, Instant},
};

use kinewright_core::{
    Analysis, Export, ExportCancellation, ExportSettings, Playback, PreviewFrame, TimeCode,
};
use serde_json::{Value, json};

use super::{
    baseline::Baseline,
    protocol::{Action, Call},
    report::Report,
};

#[must_use]
pub fn stamp_value(stamp: kinewright_core::FrameStamp) -> Value {
    json!({"epoch":stamp.epoch,"seq":stamp.seq})
}

#[derive(Default)]
pub struct MediaObserver {
    preview: Option<Vec<u8>>,
}

fn receive(engine: &kinewright_media::FfmpegMediaEngine, at: i64) -> Result<PreviewFrame, String> {
    let frames = engine.frames();
    while frames.try_recv().is_ok() {}
    engine.request_frame(TimeCode(at));
    let requested = engine.stamp();
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let frame = frames
            .recv_timeout(deadline.saturating_duration_since(Instant::now()))
            .map_err(|e| format!("media frame prerequisite/deadline: {e}"))?;
        if frame.at == TimeCode(at) && frame.stamp == requested {
            return Ok(frame);
        }
    }
}

fn frame_value(frame: &PreviewFrame, report: &mut Report, name: &str) -> Result<Value, String> {
    let pixels = frame.texture.rgba.as_slice();
    let (width, height) = (frame.texture.width, frame.texture.height);
    let center =
        usize::try_from((u64::from(height / 2) * u64::from(width) + u64::from(width / 2)) * 4)
            .map_err(|e| e.to_string())?;
    let center = pixels
        .get(center..center + 4)
        .ok_or("missing center landmark")?;
    let artifact = report.artifact(name, pixels)?;
    Ok(
        json!({"at":frame.at,"stamp":stamp_value(frame.stamp),"width":width,"height":height,"center_rgba":center,"raw_rgba":artifact,"durable_version_id":null,"decoder_snapshot_receipt":null}),
    )
}

impl MediaObserver {
    #[allow(clippy::too_many_lines)]
    pub fn execute(
        &mut self,
        action: &Action,
        baseline: &mut Baseline,
        report: &mut Report,
    ) -> Result<Option<Value>, String> {
        let call = &action.call;
        let prefix = format!("media-{}-{}", action.participant, action.id);
        if matches!(
            call,
            Call::MediaSeek(_)
                | Call::MediaProof(_)
                | Call::MediaExport(_)
                | Call::MediaDecode(_)
                | Call::MediaStats(_)
        ) {
            baseline.require_generated_media_control()?;
        }
        let value = match call {
            Call::MediaSeek(args) => {
                let engine = baseline.media()?;
                let (_, document) = baseline.snapshot()?;
                engine.set_document(document);
                engine.seek(TimeCode(args.at));
                let frame = receive(&engine, args.at)?;
                self.preview = Some(frame.texture.rgba.as_slice().to_vec());
                let mut value = frame_value(&frame, report, &format!("{prefix}-preview.rgba"))?;
                value["requested_stamp"] = stamp_value(engine.stamp());
                value
            }
            Call::MediaProof(args) => {
                let engine = baseline.media()?;
                engine.set_document(baseline.snapshot()?.1);
                let proof = engine
                    .thumbnail_at(TimeCode(args.at), args.max_width)
                    .map_err(|e| e.to_string())?;
                let artifact = report.artifact(&format!("{prefix}-proof.rgba"), &proof.pixels)?;
                json!({"at":args.at,"width":proof.width,"height":proof.height,"raw_rgba":artifact})
            }
            Call::MediaExport(args) => {
                let engine = baseline.media()?;
                let (_, document) = baseline.snapshot()?;
                let path = baseline.output_path(&args.path)?;
                let settings = ExportSettings {
                    fps: document.fps,
                    resolution: document.resolution,
                    delivery_color: document.color_context.delivery.clone(),
                    video_codec: "libx264".into(),
                    audio_codec: "aac".into(),
                    video_bitrate: 1_000_000,
                    audio_bitrate: 128_000,
                    loudness_normalization: None,
                    cancellation: ExportCancellation::default(),
                };
                let (sender, progress) = crossbeam_channel::unbounded();
                let result = engine
                    .export_document_reporting(document, &path, settings, sender)
                    .map_err(|e| e.to_string())?;
                let artifact = report.artifact_file(&format!("{prefix}-export.mp4"), &path)?;
                json!({"output":artifact,"export_report":format!("{result:?}"),"progress":progress.try_iter().map(|p| format!("{p:?}")).collect::<Vec<_>>(),"normalization_requested":false,"whole_output_qa":null})
            }
            Call::MediaDecode(args) => {
                let path = baseline.output_path(&args.path)?;
                let decoder = kinewright_media::FfmpegMediaEngine::new_with_data_dir(
                    baseline.root.join(format!("decode-cache-{}", action.id)),
                )
                .map_err(|e| format!("decode prerequisite NOT RUN: {e}"))?;
                let finished = decoder.teardown_observer();
                let result = (|| {
                    let asset = decoder.probe(&path).map_err(|e| e.to_string())?;
                    let source = json!({"path":asset.path,"fingerprint":asset.source_fingerprint,"fps":asset.fps,"duration":asset.duration});
                    decoder.set_document(Arc::new(
                        kinewright_media::test_support::single_clip_document(asset),
                    ));
                    let frame = receive(&decoder, args.at)?;
                    let preview = self
                        .preview
                        .as_ref()
                        .ok_or("no preview comparison control")?;
                    if preview.len() != frame.texture.rgba.len() {
                        return Err(
                            "decoded raster differs from preview; no byte comparison".into()
                        );
                    }
                    let maximum = preview
                        .iter()
                        .zip(frame.texture.rgba.iter())
                        .map(|(a, b)| a.abs_diff(*b))
                        .max()
                        .ok_or("empty decoded image")?;
                    let mut value = frame_value(&frame, report, &format!("{prefix}-decoded.rgba"))?;
                    value["max_channel_delta_from_preview"] = json!(maximum);
                    value["source"] = source;
                    Ok::<_, String>(value)
                })();
                drop(decoder);
                if finished.recv_timeout(Duration::from_millis(super::MEDIA_TEARDOWN_MS))
                    != Err(crossbeam_channel::RecvTimeoutError::Disconnected)
                {
                    return Err("decode worker teardown not observed".into());
                }
                let mut value = result?;
                value["decoder_worker_finished"] = json!(true);
                value
            }
            Call::MediaStats(_) => {
                let engine = baseline.media()?;
                let stats = engine.stats();
                json!({"render_adapter":engine.observed_adapter(),"sync_decoders":stats.sync_decoders,"live_table_bytes":stats.live_table_bytes,"shutdown_detached_readers":stats.shutdown_detached_readers,
                    "raw_playback_stats":format!("{stats:?}"),"rss_bytes":null,"vram_bytes":null,"pin_charge_counters":null})
            }
            _ => return Ok(None),
        };
        Ok(Some(value))
    }
}
