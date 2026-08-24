use std::{
    collections::BTreeMap,
    fs::{self, File},
    io::{Read, Write},
    ops::Range,
    path::{Path, PathBuf},
    sync::Arc,
    thread,
};

use crossbeam_channel::{Sender, unbounded};
use kinewright_core::{
    AssetId, AssetTranscript, Document, ExportCancellation, FrameRounding, MediaAsset, MediaError,
    MediaKind, Rational, TimeCode, TimelineTranscriptWord, TranscriptStatus, TranscriptWord,
    map_frames_with_rounding, map_source_range_to_project,
};
use serde::{Deserialize, Serialize};
use whisper_rs::{
    FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters, WhisperSegment,
};

use crate::{
    audio::decode_audio_range,
    derived_cache::{CancellationRegistry, JsonCache, StatusReporter, cache_root, spawn_worker},
    sha256::sha256_file,
};

const CACHE_VERSION: u32 = 2;
const WHISPER_SAMPLE_RATE: u32 = 16_000;

/// `OpenAI` Whisper `small`, converted to GGML by the whisper.cpp project. The
/// revision, bytes, and digest are pinned so first-use download is reproducible.
/// The model repository declares the converted weights under the MIT license.
pub const WHISPER_MODEL_NAME: &str = "ggml-small.bin";
pub const WHISPER_MODEL_URL: &str = "https://huggingface.co/ggerganov/whisper.cpp/resolve/90a64d80ea254cf67575b41a5971f972c79f7b45/ggml-small.bin?download=true";
pub const WHISPER_MODEL_SHA256: &str =
    "1be3a9b2063867b937e64e2ec7483364a79917e157fa98c5d94b5c1fffea987b";
pub const WHISPER_MODEL_LICENSE: &str = "MIT";

pub(crate) struct TranscriptService {
    jobs: Sender<TranscriptJob>,
    states: StatusReporter<TranscriptStatus>,
    cancellations: CancellationRegistry,
}

struct TranscriptJob {
    asset: MediaAsset,
    language: Option<String>,
    cancellation: ExportCancellation,
}

impl TranscriptService {
    pub(crate) fn new(data_dir: PathBuf) -> Result<Self, MediaError> {
        let (jobs, jobs_rx) = unbounded();
        let states = StatusReporter::new();
        let cancellations = CancellationRegistry::default();
        let mut worker = TranscriptWorker::new(data_dir, states.clone(), cancellations.clone());
        spawn_worker("kinewright-transcript", "transcript", jobs_rx, move |job| {
            worker.handle(&job)
        })?;
        Ok(Self {
            jobs,
            states,
            cancellations,
        })
    }

    pub(crate) fn request(&self, asset: MediaAsset, language: Option<String>) {
        if !matches!(asset.kind, MediaKind::Audio | MediaKind::AudioVideo) {
            self.update(&asset.path, TranscriptStatus::NoSpeech);
            return;
        }
        let should_queue = self.states.should_queue(&asset.path, |status| {
            matches!(
                status,
                TranscriptStatus::Queued
                    | TranscriptStatus::Hashing
                    | TranscriptStatus::DownloadingModel { .. }
                    | TranscriptStatus::Transcribing { .. }
                    | TranscriptStatus::Ready(_)
                    | TranscriptStatus::NoSpeech
            )
        });
        if !should_queue {
            return;
        }
        let Some(cancellation) = self.cancellations.start(&asset.path) else {
            return;
        };
        self.update(&asset.path, TranscriptStatus::Queued);
        let path = asset.path.clone();
        if self
            .jobs
            .send(TranscriptJob {
                asset,
                language,
                cancellation: cancellation.clone(),
            })
            .is_err()
        {
            self.cancellations.finish(&path, &cancellation);
            self.update(
                &path,
                TranscriptStatus::Failed("transcript worker stopped".to_owned()),
            );
        }
    }

    pub(crate) fn status(&self, path: &Path) -> TranscriptStatus {
        self.states.get_or(path, TranscriptStatus::NotRequested)
    }

    pub(crate) fn cancel(&self, path: &Path) -> bool {
        let cancelled = self.cancellations.cancel(path);
        if cancelled {
            self.update(path, TranscriptStatus::Cancelled);
        }
        cancelled
    }

    pub(crate) fn register(&self, path: &Path, transcript: AssetTranscript) {
        self.update(path, TranscriptStatus::Ready(Arc::new(transcript)));
    }

    pub(crate) fn timeline_words(
        &self,
        document: &Document,
        range: Option<Range<TimeCode>>,
    ) -> Result<Vec<TimelineTranscriptWord>, MediaError> {
        map_timeline_words(document, range, |asset| match self.status(&asset.path) {
            TranscriptStatus::Ready(transcript) => Some(transcript),
            _ => None,
        })
    }

    fn update(&self, path: &Path, status: TranscriptStatus) {
        self.states.update(path, status);
    }
}

struct TranscriptWorker {
    data_dir: PathBuf,
    states: StatusReporter<TranscriptStatus>,
    store: TranscriptStore,
    model: Option<WhisperContext>,
    cancellations: CancellationRegistry,
}

impl TranscriptWorker {
    fn new(
        data_dir: PathBuf,
        states: StatusReporter<TranscriptStatus>,
        cancellations: CancellationRegistry,
    ) -> Self {
        Self {
            store: TranscriptStore::new(cache_root(&data_dir, "transcripts", CACHE_VERSION)),
            data_dir,
            states,
            model: None,
            cancellations,
        }
    }

    fn handle(&mut self, job: &TranscriptJob) -> bool {
        if let Err(error) =
            self.transcribe_asset(&job.asset, job.language.as_deref(), &job.cancellation)
        {
            let status = if error == MediaError::Cancelled {
                TranscriptStatus::Cancelled
            } else {
                TranscriptStatus::Failed(error.to_string())
            };
            self.update(&job.asset.path, status);
        }
        self.cancellations
            .finish(&job.asset.path, &job.cancellation);
        true
    }

    fn transcribe_asset(
        &mut self,
        asset: &MediaAsset,
        language: Option<&str>,
        cancellation: &ExportCancellation,
    ) -> Result<(), MediaError> {
        cancelled(cancellation)?;
        self.update(&asset.path, TranscriptStatus::Hashing);
        let content_sha256 = sha256_file(&asset.path)?;
        cancelled(cancellation)?;
        if let Some(cached) = self.store.load(&content_sha256, asset.id)? {
            cancelled(cancellation)?;
            self.finish(&asset.path, cached);
            return Ok(());
        }

        if self.model.is_none() {
            let model_path = ensure_model(&self.data_dir, &asset.path, &self.states, cancellation)?;
            cancelled(cancellation)?;
            self.model = Some(load_whisper_context(&model_path)?);
        }

        self.update(
            &asset.path,
            TranscriptStatus::Transcribing {
                progress_percent: 0,
            },
        );
        let samples = decode_audio_range(
            &asset.path,
            asset.fps,
            TimeCode::ZERO,
            asset.duration,
            WHISPER_SAMPLE_RATE,
            1,
            cancellation,
        )?;
        cancelled(cancellation)?;
        let context = self
            .model
            .as_ref()
            .ok_or_else(|| MediaError::Backend("Whisper model was not loaded".to_owned()))?;
        let transcript = run_whisper(
            context,
            &samples,
            asset,
            content_sha256,
            self.states.clone(),
            cancellation,
            language,
        )?;
        cancelled(cancellation)?;
        self.store.save(&transcript)?;
        cancelled(cancellation)?;
        self.finish(&asset.path, transcript);
        Ok(())
    }

    fn finish(&self, path: &Path, transcript: AssetTranscript) {
        if transcript.words.is_empty() {
            self.update(path, TranscriptStatus::NoSpeech);
        } else {
            self.update(path, TranscriptStatus::Ready(Arc::new(transcript)));
        }
    }

    fn update(&self, path: &Path, status: TranscriptStatus) {
        self.states.update(path, status);
    }
}

fn run_whisper(
    context: &WhisperContext,
    samples: &[f32],
    asset: &MediaAsset,
    content_sha256: String,
    states: StatusReporter<TranscriptStatus>,
    cancellation: &ExportCancellation,
    language: Option<&str>,
) -> Result<AssetTranscript, MediaError> {
    let mut state = context
        .create_state()
        .map_err(|error| MediaError::Backend(format!("could not create Whisper state: {error}")))?;
    let mut params = FullParams::new(SamplingStrategy::BeamSearch {
        beam_size: 5,
        patience: -1.0,
    });
    let threads = thread::available_parallelism()
        .map_or(4, std::num::NonZeroUsize::get)
        .min(8);
    params.set_n_threads(i32::try_from(threads).unwrap_or(4));
    params.set_translate(false);
    params.set_language(language);
    params.set_print_special(false);
    params.set_print_progress(false);
    params.set_print_realtime(false);
    params.set_print_timestamps(false);
    params.set_token_timestamps(true);
    params.set_split_on_word(true);
    params.set_max_len(1);
    let progress_path = asset.path.clone();
    let progress_cancellation = cancellation.clone();
    params.set_progress_callback_safe(move |progress: i32| {
        if progress_cancellation.is_cancelled() {
            return;
        }
        let clamped = progress.clamp(0, 100);
        states.update(
            &progress_path,
            TranscriptStatus::Transcribing {
                progress_percent: u8::try_from(clamped).unwrap_or(100),
            },
        );
    });
    // whisper-rs 0.16's safe abort wrapper instantiates its C trampoline with
    // a pointer type different from the erased closure it stores. On Windows
    // this spuriously aborts healthy graphs. Cancellation remains checked
    // immediately before and after the synchronous inference boundary.
    if let Err(error) = state.full(params, samples) {
        return if cancellation.is_cancelled() {
            Err(MediaError::Cancelled)
        } else {
            Err(MediaError::Backend(format!(
                "Whisper inference failed: {error}"
            )))
        };
    }
    cancelled(cancellation)?;

    let words = extract_words(
        &state.as_iter().collect::<Vec<_>>(),
        asset.fps,
        asset.duration,
    )?;
    Ok(AssetTranscript {
        asset: asset.id,
        content_sha256,
        source_fps: asset.fps,
        words,
    })
}

fn load_whisper_context(model_path: &Path) -> Result<WhisperContext, MediaError> {
    let model_path = model_path.to_string_lossy();
    WhisperContext::new_with_params(model_path.as_ref(), WhisperContextParameters::default())
        .map_err(|error| MediaError::Backend(format!("could not load Whisper model: {error}")))
}

fn extract_words(
    segments: &[WhisperSegment<'_>],
    fps: Rational,
    duration: TimeCode,
) -> Result<Vec<TranscriptWord>, MediaError> {
    let mut result = Vec::new();
    for segment in segments {
        let mut tokens = Vec::new();
        for token_index in 0..segment.n_tokens() {
            let Some(token) = segment.get_token(token_index) else {
                continue;
            };
            let data = token.token_data();
            let text = token
                .to_str_lossy()
                .map_err(|error| MediaError::Backend(format!("invalid Whisper token: {error}")))?;
            let trimmed = text.trim();
            if data.t0 < 0
                || data.t1 <= data.t0
                || trimmed.is_empty()
                || (trimmed.starts_with("<|") && trimmed.ends_with("|>"))
            {
                continue;
            }
            tokens.push((text.into_owned(), data.t0, data.t1));
        }
        let before = result.len();
        append_token_words(&mut result, &tokens, fps, duration)?;
        if result.len() == before {
            let text = segment.to_str_lossy().map_err(|error| {
                MediaError::Backend(format!("invalid Whisper segment: {error}"))
            })?;
            append_evenly_timed_words(
                &mut result,
                &text,
                segment.start_timestamp(),
                segment.end_timestamp(),
                fps,
                duration,
            )?;
        }
    }
    result.retain(|word| word.source_end > word.source_start && !word.text.is_empty());
    Ok(result)
}

fn append_token_words(
    output: &mut Vec<TranscriptWord>,
    tokens: &[(String, i64, i64)],
    fps: Rational,
    duration: TimeCode,
) -> Result<(), MediaError> {
    let mut pending: Option<(String, i64, i64)> = None;
    for (raw, start, end) in tokens {
        let begins_word = raw.chars().next().is_some_and(char::is_whitespace);
        if begins_word {
            flush_pending(output, &mut pending, fps, duration)?;
        }
        let pieces = raw.split_whitespace().collect::<Vec<_>>();
        if pieces.len() > 1 {
            flush_pending(output, &mut pending, fps, duration)?;
            let span = end.saturating_sub(*start).max(1);
            for (index, piece) in pieces.iter().enumerate() {
                let piece_start = start.saturating_add(
                    span.saturating_mul(i64::try_from(index).unwrap_or_default())
                        / i64::try_from(pieces.len()).unwrap_or(1),
                );
                let piece_end = start.saturating_add(
                    span.saturating_mul(i64::try_from(index + 1).unwrap_or(1))
                        / i64::try_from(pieces.len()).unwrap_or(1),
                );
                pending = Some(((*piece).to_owned(), piece_start, piece_end));
                flush_pending(output, &mut pending, fps, duration)?;
            }
            continue;
        }
        let text = raw.trim();
        if text.is_empty() {
            continue;
        }
        if let Some((pending_text, _, pending_end)) = &mut pending {
            pending_text.push_str(text);
            *pending_end = (*pending_end).max(*end);
        } else {
            pending = Some((text.to_owned(), *start, *end));
        }
    }
    flush_pending(output, &mut pending, fps, duration)
}

fn append_evenly_timed_words(
    output: &mut Vec<TranscriptWord>,
    text: &str,
    start: i64,
    end: i64,
    fps: Rational,
    duration: TimeCode,
) -> Result<(), MediaError> {
    let words = text.split_whitespace().collect::<Vec<_>>();
    if words.is_empty() || end <= start {
        return Ok(());
    }
    let span = end - start;
    let count = i64::try_from(words.len()).unwrap_or(1);
    for (index, word) in words.iter().enumerate() {
        let index = i64::try_from(index).unwrap_or_default();
        push_word(
            output,
            word,
            start + span * index / count,
            start + span * (index + 1) / count,
            fps,
            duration,
        )?;
    }
    Ok(())
}

fn flush_pending(
    output: &mut Vec<TranscriptWord>,
    pending: &mut Option<(String, i64, i64)>,
    fps: Rational,
    duration: TimeCode,
) -> Result<(), MediaError> {
    if let Some((text, start, end)) = pending.take() {
        push_word(output, &text, start, end, fps, duration)?;
    }
    Ok(())
}

fn push_word(
    output: &mut Vec<TranscriptWord>,
    text: &str,
    start_centiseconds: i64,
    end_centiseconds: i64,
    fps: Rational,
    duration: TimeCode,
) -> Result<(), MediaError> {
    let text = text.trim();
    if text.is_empty() || (text.starts_with("<|") && text.ends_with("|>")) {
        return Ok(());
    }
    // Word endpoints are shared edit boundaries. Map both sides with the same
    // rounding rule so adjacent tokens stay adjacent after conversion to the
    // asset's integer frame time base.
    let start = centiseconds_to_frames(start_centiseconds, fps, FrameRounding::Nearest)?;
    let end = centiseconds_to_frames(end_centiseconds, fps, FrameRounding::Nearest)?;
    let source_start = TimeCode(start.0.clamp(0, duration.0));
    if source_start >= duration {
        return Ok(());
    }
    let source_end = TimeCode(end.0.clamp(source_start.0.saturating_add(1), duration.0));
    if source_end <= source_start {
        return Ok(());
    }
    output.push(TranscriptWord {
        text: text.to_owned(),
        source_start,
        source_end,
        speaker: None,
    });
    Ok(())
}

fn centiseconds_to_frames(
    centiseconds: i64,
    fps: Rational,
    rounding: FrameRounding,
) -> Result<TimeCode, MediaError> {
    if centiseconds < 0 {
        return Ok(TimeCode::ZERO);
    }
    let centisecond_rate =
        Rational::new(100, 1).map_err(|error| MediaError::Backend(error.to_string()))?;
    map_frames_with_rounding(TimeCode(centiseconds), centisecond_rate, fps, rounding)
        .map_err(|error| MediaError::Backend(error.to_string()))
}

pub(crate) fn map_timeline_words<F>(
    document: &Document,
    range: Option<Range<TimeCode>>,
    mut transcript_for: F,
) -> Result<Vec<TimelineTranscriptWord>, MediaError>
where
    F: FnMut(&MediaAsset) -> Option<Arc<AssetTranscript>>,
{
    let requested = range.unwrap_or(TimeCode::ZERO..document.duration);
    if requested.start < TimeCode::ZERO || requested.end <= requested.start {
        return Err(MediaError::Backend(format!(
            "timeline transcript range must be non-empty and non-negative: {}..{}",
            requested.start.0, requested.end.0
        )));
    }
    let mut transcripts = BTreeMap::new();
    let mut words = Vec::new();
    for track in &document.tracks {
        for clip in &track.clips {
            if !clip.content.is_media() {
                continue;
            }
            // Derived source timestamps no longer align project-linearly on a
            // speed-changed clip; remapping them is deferred, so skip for now.
            if clip.speed_percent != 100 {
                continue;
            }
            let Some(asset) = document.asset(clip.asset) else {
                continue;
            };
            let transcript = transcripts
                .entry(asset.id)
                .or_insert_with(|| transcript_for(asset));
            let Some(transcript) = transcript else {
                continue;
            };
            let clip_duration =
                map_source_range_to_project(clip.source_range.clone(), asset.fps, document.fps)
                    .map_err(|error| MediaError::Backend(error.to_string()))?;
            let clip_end = clip
                .timeline_start
                .checked_add(clip_duration)
                .ok_or_else(|| MediaError::Backend("timeline position overflowed".to_owned()))?;
            if clip.timeline_start >= requested.end || clip_end <= requested.start {
                continue;
            }
            for word in &transcript.words {
                let source_start = word.source_start.max(clip.source_range.start);
                let source_end = word.source_end.min(clip.source_range.end);
                if source_end <= source_start {
                    continue;
                }
                let start_offset = source_start
                    .checked_sub(clip.source_range.start)
                    .ok_or_else(|| MediaError::Backend("source position underflowed".to_owned()))?;
                let end_offset = source_end
                    .checked_sub(clip.source_range.start)
                    .ok_or_else(|| MediaError::Backend("source position underflowed".to_owned()))?;
                let mapped_start = map_frames_with_rounding(
                    start_offset,
                    asset.fps,
                    document.fps,
                    FrameRounding::Floor,
                )
                .map_err(|error| MediaError::Backend(error.to_string()))?;
                let mapped_end = map_frames_with_rounding(
                    end_offset,
                    asset.fps,
                    document.fps,
                    FrameRounding::Ceil,
                )
                .map_err(|error| MediaError::Backend(error.to_string()))?;
                let project_start =
                    clip.timeline_start
                        .checked_add(mapped_start)
                        .ok_or_else(|| {
                            MediaError::Backend("timeline position overflowed".to_owned())
                        })?;
                let project_end = clip
                    .timeline_start
                    .checked_add(mapped_end)
                    .ok_or_else(|| MediaError::Backend("timeline position overflowed".to_owned()))?
                    .min(clip_end);
                if project_end <= requested.start
                    || project_start >= requested.end
                    || project_end <= project_start
                {
                    continue;
                }
                words.push(TimelineTranscriptWord {
                    text: word.text.clone(),
                    speaker: word.speaker.clone(),
                    asset: asset.id,
                    track: track.id,
                    clip: clip.id,
                    source_start,
                    source_end,
                    project_start,
                    project_end,
                });
            }
        }
    }
    words.sort_by_key(|word| (word.project_start, word.track, word.clip, word.source_start));
    Ok(words)
}

#[derive(Debug, Serialize, Deserialize)]
struct StoredTranscript {
    content_sha256: String,
    model_sha256: String,
    source_fps: Rational,
    words: Vec<TranscriptWord>,
}

struct TranscriptStore {
    cache: JsonCache,
}

impl TranscriptStore {
    fn new(root: PathBuf) -> Self {
        Self {
            cache: JsonCache::new(root, CACHE_VERSION, "transcript"),
        }
    }

    #[cfg(test)]
    fn path_for(&self, content_sha256: &str) -> PathBuf {
        self.cache.path_for(content_sha256, "")
    }

    fn load(
        &self,
        content_sha256: &str,
        asset: AssetId,
    ) -> Result<Option<AssetTranscript>, MediaError> {
        self.cache.load(
            &self.cache.path_for(content_sha256, ""),
            |stored: StoredTranscript| {
                (stored.content_sha256 == content_sha256
                    && stored.model_sha256 == WHISPER_MODEL_SHA256)
                    .then_some(AssetTranscript {
                        asset,
                        content_sha256: stored.content_sha256,
                        source_fps: stored.source_fps,
                        words: stored.words,
                    })
            },
        )
    }

    fn save(&self, transcript: &AssetTranscript) -> Result<(), MediaError> {
        self.cache.save(
            &self.cache.path_for(&transcript.content_sha256, ""),
            &StoredTranscript {
                content_sha256: transcript.content_sha256.clone(),
                model_sha256: WHISPER_MODEL_SHA256.to_owned(),
                source_fps: transcript.source_fps,
                words: transcript.words.clone(),
            },
        )
    }
}

fn ensure_model(
    data_dir: &Path,
    asset_path: &Path,
    states: &StatusReporter<TranscriptStatus>,
    cancellation: &ExportCancellation,
) -> Result<PathBuf, MediaError> {
    cancelled(cancellation)?;
    let model_dir = data_dir.join("models").join("whisper");
    let model_path = model_dir.join(WHISPER_MODEL_NAME);
    if model_path.is_file() && sha256_file(&model_path)? == WHISPER_MODEL_SHA256 {
        cancelled(cancellation)?;
        return Ok(model_path);
    }
    fs::create_dir_all(&model_dir).map_err(|error| {
        MediaError::Backend(format!("could not create model directory: {error}"))
    })?;
    let temporary = model_dir.join(format!("{WHISPER_MODEL_NAME}.part-{}", std::process::id()));
    let mut response = reqwest::blocking::Client::builder()
        .build()
        .and_then(|client| client.get(WHISPER_MODEL_URL).send())
        .map_err(|error| {
            MediaError::Backend(format!("could not download Whisper model: {error}"))
        })?;
    if !response.status().is_success() {
        return Err(MediaError::Backend(format!(
            "Whisper model download returned HTTP {}",
            response.status()
        )));
    }
    let total = response.content_length();
    states.update(
        asset_path,
        TranscriptStatus::DownloadingModel {
            downloaded_bytes: 0,
            total_bytes: total,
        },
    );
    let mut file = File::create(&temporary).map_err(|error| {
        MediaError::Backend(format!("could not create model download: {error}"))
    })?;
    let mut buffer = vec![0_u8; 1024 * 1024];
    let mut downloaded = 0_u64;
    loop {
        if cancellation.is_cancelled() {
            let _ = fs::remove_file(&temporary);
            return Err(MediaError::Cancelled);
        }
        let count = response.read(&mut buffer).map_err(|error| {
            MediaError::Backend(format!("could not read model download: {error}"))
        })?;
        if count == 0 {
            break;
        }
        file.write_all(&buffer[..count]).map_err(|error| {
            MediaError::Backend(format!("could not write model download: {error}"))
        })?;
        downloaded = downloaded.saturating_add(u64::try_from(count).unwrap_or_default());
        states.update(
            asset_path,
            TranscriptStatus::DownloadingModel {
                downloaded_bytes: downloaded,
                total_bytes: total,
            },
        );
    }
    file.sync_all()
        .map_err(|error| MediaError::Backend(format!("could not flush model download: {error}")))?;
    cancelled(cancellation)?;
    let actual = sha256_file(&temporary)?;
    if actual != WHISPER_MODEL_SHA256 {
        let _ = fs::remove_file(&temporary);
        return Err(MediaError::Backend(format!(
            "Whisper model SHA-256 mismatch: expected {WHISPER_MODEL_SHA256}, got {actual}"
        )));
    }
    if model_path.exists() {
        fs::remove_file(&model_path).map_err(|error| {
            MediaError::Backend(format!("could not replace Whisper model: {error}"))
        })?;
    }
    fs::rename(&temporary, &model_path).map_err(|error| {
        MediaError::Backend(format!("could not install Whisper model: {error}"))
    })?;
    Ok(model_path)
}

fn cancelled(cancellation: &ExportCancellation) -> Result<(), MediaError> {
    if cancellation.is_cancelled() {
        Err(MediaError::Cancelled)
    } else {
        Ok(())
    }
}

#[must_use]
pub fn default_data_dir() -> PathBuf {
    std::env::var_os("LOCALAPPDATA").map_or_else(
        || std::env::temp_dir().join("Kinewright"),
        |root| PathBuf::from(root).join("Kinewright"),
    )
}

#[cfg(test)]
mod tests {
    use kinewright_core::{Clip, ClipContent, ClipId, Track, TrackId, TrackKind};

    use super::*;
    use crate::test_support::TempDirectory;

    fn fixture_transcript(asset: AssetId, hash: &str, fps: Rational) -> AssetTranscript {
        AssetTranscript {
            asset,
            content_sha256: hash.to_owned(),
            source_fps: fps,
            words: vec![TranscriptWord {
                text: "hello".to_owned(),
                source_start: TimeCode(24),
                source_end: TimeCode(36),
                speaker: Some("speaker-a".to_owned()),
            }],
        }
    }

    #[test]
    fn transcript_cache_round_trips_and_rebinds_asset_id() {
        let directory = TempDirectory::new("transcript-cache-roundtrip");
        let store = TranscriptStore::new(directory.root().to_path_buf());
        let transcript = fixture_transcript(
            AssetId(1),
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            Rational::new(24, 1).unwrap(),
        );
        store.save(&transcript).unwrap();

        let loaded = store
            .load(&transcript.content_sha256, AssetId(99))
            .unwrap()
            .unwrap();
        assert_eq!(loaded.asset, AssetId(99));
        assert_eq!(loaded.words, transcript.words);
        assert_eq!(loaded.source_fps, transcript.source_fps);
    }

    #[test]
    fn registered_sidecar_transcript_is_immediately_available_for_the_session() {
        let directory = TempDirectory::new("registered-transcript");
        let service = TranscriptService::new(directory.root().to_path_buf()).unwrap();
        let path = directory.path("camera.avi");
        let transcript = fixture_transcript(
            AssetId(4),
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            Rational::new(25, 1).unwrap(),
        );

        service.register(&path, transcript.clone());

        let TranscriptStatus::Ready(observed) = service.status(&path) else {
            panic!("registered transcript should be ready");
        };
        assert_eq!(observed.as_ref(), &transcript);
    }

    #[test]
    fn content_hash_is_the_cache_key() {
        let directory = TempDirectory::new("transcript-cache-key");
        let store = TranscriptStore::new(directory.root().to_path_buf());
        let first_hash = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
        let second_hash = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
        store
            .save(&fixture_transcript(
                AssetId(1),
                first_hash,
                Rational::new(30, 1).unwrap(),
            ))
            .unwrap();

        assert!(store.load(first_hash, AssetId(2)).unwrap().is_some());
        assert!(store.load(second_hash, AssetId(2)).unwrap().is_none());
        assert_ne!(store.path_for(first_hash), store.path_for(second_hash));
    }

    #[test]
    fn corrupt_cache_entry_is_ignored_as_a_miss() {
        let directory = TempDirectory::new("transcript-cache-corrupt");
        let store = TranscriptStore::new(directory.root().to_path_buf());
        let hash = "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc";
        fs::create_dir_all(directory.root()).unwrap();
        fs::write(store.path_for(hash), b"not json").unwrap();

        assert!(store.load(hash, AssetId(1)).unwrap().is_none());
    }

    #[test]
    fn source_words_map_through_clip_boundaries() {
        let document = fixture_document(Rational::new(30, 1).unwrap());
        let transcript = Arc::new(AssetTranscript {
            asset: AssetId(1),
            content_sha256: "fixture".to_owned(),
            source_fps: Rational::new(30, 1).unwrap(),
            words: vec![
                TranscriptWord {
                    text: "before".to_owned(),
                    source_start: TimeCode(8),
                    source_end: TimeCode(12),
                    speaker: None,
                },
                TranscriptWord {
                    text: "inside".to_owned(),
                    source_start: TimeCode(15),
                    source_end: TimeCode(18),
                    speaker: Some("speaker-a".to_owned()),
                },
                TranscriptWord {
                    text: "after".to_owned(),
                    source_start: TimeCode(39),
                    source_end: TimeCode(42),
                    speaker: None,
                },
            ],
        });
        let mapped =
            map_timeline_words(&document, None, |_| Some(Arc::clone(&transcript))).unwrap();

        assert_eq!(mapped.len(), 3);
        assert_eq!(mapped[0].source_start, TimeCode(10));
        assert_eq!(mapped[0].project_start, TimeCode(100));
        assert_eq!(
            mapped[1].project_start..mapped[1].project_end,
            TimeCode(105)..TimeCode(108)
        );
        assert_eq!(mapped[2].source_end, TimeCode(40));
        assert_eq!(mapped[2].project_end, TimeCode(130));
    }

    #[test]
    fn mixed_fps_mapping_uses_floor_start_and_ceil_end() {
        let document = fixture_document(Rational::new(24, 1).unwrap());
        let transcript = Arc::new(AssetTranscript {
            asset: AssetId(1),
            content_sha256: "fixture".to_owned(),
            source_fps: Rational::new(24, 1).unwrap(),
            words: vec![TranscriptWord {
                text: "hello".to_owned(),
                source_start: TimeCode(11),
                source_end: TimeCode(13),
                speaker: None,
            }],
        });
        let mapped =
            map_timeline_words(&document, None, |_| Some(Arc::clone(&transcript))).unwrap();

        assert_eq!(mapped[0].project_start, TimeCode(101));
        assert_eq!(mapped[0].project_end, TimeCode(104));
    }

    #[test]
    fn adjacent_token_boundaries_share_the_same_source_frame() {
        let fps = Rational::new(30, 1).unwrap();
        let mut words = Vec::new();
        push_word(&mut words, "hello", 16, 46, fps, TimeCode(120)).unwrap();
        push_word(&mut words, "um", 46, 82, fps, TimeCode(120)).unwrap();

        assert_eq!(words[0].source_end, words[1].source_start);
        assert_eq!(words[0].source_end, TimeCode(14));
    }

    #[test]
    fn timeline_range_is_half_open() {
        let document = fixture_document(Rational::new(30, 1).unwrap());
        let transcript = Arc::new(AssetTranscript {
            asset: AssetId(1),
            content_sha256: "fixture".to_owned(),
            source_fps: Rational::new(30, 1).unwrap(),
            words: vec![TranscriptWord {
                text: "hello".to_owned(),
                source_start: TimeCode(15),
                source_end: TimeCode(18),
                speaker: None,
            }],
        });

        assert!(
            map_timeline_words(&document, Some(TimeCode(108)..TimeCode(110)), |_| Some(
                Arc::clone(&transcript)
            ))
            .unwrap()
            .is_empty()
        );
        assert_eq!(
            map_timeline_words(&document, Some(TimeCode(107)..TimeCode(110)), |_| Some(
                Arc::clone(&transcript)
            ))
            .unwrap()
            .len(),
            1
        );
    }

    fn fixture_document(source_fps: Rational) -> Document {
        let asset = MediaAsset {
            id: AssetId(1),
            path: PathBuf::from("fixture.mp4"),
            name: "fixture".to_owned(),
            duration: TimeCode(100),
            fps: source_fps,
            kind: MediaKind::AudioVideo,
            resolution: Some((320, 180)),
            color_description: kinewright_core::ColorDescription::default(),
        };
        let clip_duration = map_source_range_to_project(
            TimeCode(10)..TimeCode(40),
            source_fps,
            Rational::new(30, 1).unwrap(),
        )
        .unwrap();
        Document {
            catalog: kinewright_core::MediaCatalog::default(),
            audio_mix: kinewright_core::AudioMix::default(),
            color_context: kinewright_core::ColorContext::default(),
            tracks: vec![Track {
                id: TrackId(7),
                kind: TrackKind::Video,
                sync_lock: true,
                clips: vec![Clip {
                    id: ClipId(9),
                    asset: asset.id,
                    source_range: TimeCode(10)..TimeCode(40),
                    content: ClipContent::Media,
                    timeline_start: TimeCode(100),
                    effects: Vec::new(),
                    transition_in: None,
                    link: None,
                    audio_gain_tenth_db: 0,
                    audio_fade_in_frames: TimeCode::ZERO,
                    audio_fade_out_frames: TimeCode::ZERO,
                    speed_percent: 100,
                }],
            }],
            media_pool: vec![asset],
            markers: Vec::new(),
            fps: Rational::new(30, 1).unwrap(),
            resolution: (320, 180),
            duration: TimeCode(100 + clip_duration.0),
        }
    }
}
