use std::{
    collections::BTreeMap,
    ops::Range,
    path::{Path, PathBuf},
    sync::Arc,
};

#[cfg(any(test, feature = "test-util"))]
use crossbeam_channel::Sender;
use crossbeam_channel::unbounded;
use kinewright_core::{
    AssetId, AssetTranscript, Document, ExportCancellation, FrameRounding, MediaAsset, MediaError,
    MediaKind, Rational, TimeCode, TimelineTranscriptWord, TranscriptStatus, TranscriptWord,
    map_frames_with_rounding, map_source_range_to_project,
};
use serde::{Deserialize, Serialize};

use crate::{
    derived_cache::{CancellationRegistry, JsonCache, StatusReporter, cache_root, spawn_worker},
    sha256::sha256_file,
};

const CACHE_VERSION: u32 = 4;
use crate::recognizer::{self, ENGLISH_MODEL, Identity, Recognition};
#[cfg(any(test, feature = "test-util"))]
type InferenceGate = Arc<std::sync::Mutex<Option<(Sender<()>, crossbeam_channel::Receiver<()>)>>>;

/// A transcript job held after opening its decoder, without loading a model.
#[cfg(any(test, feature = "test-util"))]
pub struct TranscriptInferenceHold {
    arrived: crossbeam_channel::Receiver<()>,
    _release: Sender<()>,
}
#[cfg(any(test, feature = "test-util"))]
impl TranscriptInferenceHold {
    /// Wait until the real transcript worker reaches inference.
    /// # Panics
    /// Panics if the worker does not arrive within thirty seconds.
    pub fn wait(&self) {
        self.arrived
            .recv_timeout(std::time::Duration::from_secs(30))
            .expect("transcript reached inference");
    }
}

pub(crate) struct TranscriptService {
    jobs: crate::worker::OwnedWorker<crate::locator::LocatorJob<TranscriptJob>>,
    states: StatusReporter<TranscriptStatus>,
    cancellations: CancellationRegistry,
    root: PathBuf,
    #[cfg(any(test, feature = "test-util"))]
    inference_gate: InferenceGate,
}

struct TranscriptJob {
    asset: MediaAsset,
    language: Option<String>,
    cancellation: ExportCancellation,
}

impl Drop for TranscriptService {
    fn drop(&mut self) {
        self.cancellations.shutdown();
    }
}

impl TranscriptService {
    pub(crate) fn new(data_dir: PathBuf) -> Result<Self, MediaError> {
        let data_dir = crate::data_dir::ensure_root(data_dir)?;
        let (jobs, jobs_rx) = unbounded();
        let states = StatusReporter::new();
        let cancellations = CancellationRegistry::default();
        let root = cache_root(&data_dir, "transcripts", CACHE_VERSION);
        let mut worker = TranscriptWorker::new(data_dir, states.clone(), cancellations.clone());
        #[cfg(any(test, feature = "test-util"))]
        let inference_gate = Arc::clone(&worker.inference_gate);
        let jobs = spawn_worker(
            "kinewright-transcript",
            "transcript",
            (jobs, jobs_rx),
            move |job: crate::locator::LocatorJob<TranscriptJob>| {
                job.run(
                    &mut worker,
                    |worker, job| worker.handle(&job),
                    |worker, job, error| {
                        worker.update(&job.asset.path, TranscriptStatus::Failed(error.to_string()));
                        worker
                            .cancellations
                            .finish(&job.asset.path, &job.cancellation);
                        true
                    },
                )
            },
        )?;
        Ok(Self {
            jobs,
            states,
            cancellations,
            root,
            #[cfg(any(test, feature = "test-util"))]
            inference_gate,
        })
    }

    #[cfg(any(test, feature = "test-util"))]
    pub(crate) fn hold_inference(&self) -> TranscriptInferenceHold {
        let (arrived, arrival) = crossbeam_channel::bounded(1);
        let (release, released) = crossbeam_channel::bounded(1);
        *self.inference_gate.lock().expect("inference gate") = Some((arrived, released));
        TranscriptInferenceHold {
            arrived: arrival,
            _release: release,
        }
    }

    pub(crate) fn request(
        &self,
        asset: MediaAsset,
        language: Option<String>,
        grant: kinewright_core::LegacyMediaGrant,
    ) {
        let _authority = crate::locator::LocatorAuthority::enter(&grant);
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
            .send(crate::locator::LocatorJob::new(
                grant,
                TranscriptJob {
                    asset,
                    language,
                    cancellation: cancellation.clone(),
                },
            ))
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

    pub(crate) fn retire(&self, authority: kinewright_core::LegacyMediaIdentity) {
        self.cancellations.retire(authority);
        self.states.retire(authority);
    }

    pub(crate) fn cancel_all(&self) {
        self.cancellations.cancel_all();
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

    pub(crate) fn cache_stats(&self) -> Result<crate::derived_cache::CacheStats, MediaError> {
        crate::derived_cache::inventory_cache_root(&self.root)
    }

    pub(crate) fn cache_root(&self) -> &Path {
        &self.root
    }

    pub(crate) fn clear_cache(&self) -> Result<crate::derived_cache::CacheStats, MediaError> {
        crate::derived_cache::clear_cache_root(&self.root)
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
    #[cfg(any(test, feature = "test-util"))]
    inference_gate: InferenceGate,
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
            #[cfg(any(test, feature = "test-util"))]
            inference_gate: Arc::default(),
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
        let identity = Identity::english();
        if language.is_some_and(|value| !value.is_empty() && !value.eq_ignore_ascii_case("en")) {
            return Err(recognizer::Error::UnsupportedLanguage.into());
        }
        if let Some(cached) = self.store.load(
            &content_sha256,
            &identity,
            asset.fps,
            asset.duration,
            language,
        )? {
            cancelled(cancellation)?;
            self.finish(&asset.path, cached.into_asset(asset.id)?);
            return Ok(());
        }

        #[cfg(any(test, feature = "test-util"))]
        self.hold_before_inference(asset, cancellation)?;

        let states = self.states.clone();
        let asset_path = asset.path.clone();
        let model_root = recognizer::provision(
            &ENGLISH_MODEL,
            &self.data_dir,
            cancellation,
            move |downloaded_bytes, total_bytes| {
                states.update(
                    &asset_path,
                    TranscriptStatus::DownloadingModel {
                        downloaded_bytes,
                        total_bytes,
                    },
                );
            },
        )
        .map_err(MediaError::from)?;
        self.update(
            &asset.path,
            TranscriptStatus::Transcribing {
                progress_percent: 0,
            },
        );
        let states = self.states.clone();
        let asset_path = asset.path.clone();
        let source_us = source_duration_us(asset.duration, asset.fps)?;
        let recognized = recognizer::queue::file(
            &self.data_dir,
            &model_root,
            asset.path.clone(),
            asset.duration,
            asset.fps,
            cancellation.clone(),
            identity,
            move |processed| {
                let progress_percent =
                    u8::try_from(processed.saturating_mul(100) / source_us.max(1))
                        .unwrap_or(100)
                        .min(100);
                states.update(
                    &asset_path,
                    TranscriptStatus::Transcribing { progress_percent },
                );
            },
        )
        .map_err(MediaError::from);
        let completed = recognized.map(|recognized| StoredTranscript {
            content_sha256,
            identity: recognized.identity,
            source_fps: asset.fps,
            source_frames: asset.duration,
            language: language.map(str::to_owned),
            recognition: recognized.recognition,
        });
        let stored = self.store.save_completed(completed, cancellation)?;
        cancelled(cancellation)?;
        self.finish(&asset.path, stored.into_asset(asset.id)?);
        Ok(())
    }

    #[cfg(any(test, feature = "test-util"))]
    fn hold_before_inference(
        &self,
        asset: &MediaAsset,
        cancellation: &ExportCancellation,
    ) -> Result<(), MediaError> {
        let gate = self.inference_gate.lock().expect("inference gate").take();
        if let Some((arrived, released)) = gate {
            let end =
                crate::clock::frame_to_samples(asset.duration, recognizer::SAMPLE_RATE, asset.fps);
            let mut decoder =
                crate::audio::AudioDecoder::open(&asset.path, recognizer::SAMPLE_RATE, 1, 0, end)?;
            assert!(
                decoder.next_chunk()?.is_some(),
                "held transcript decoded real audio"
            );
            let _ = arrived.send(());
            loop {
                if cancelled(cancellation).is_err() {
                    return Err(MediaError::Cancelled);
                }
                match released.recv_timeout(std::time::Duration::from_millis(5)) {
                    Err(crossbeam_channel::RecvTimeoutError::Timeout) => {}
                    _ => return Err(MediaError::Cancelled),
                }
            }
        }
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

fn source_duration_us(frames: TimeCode, fps: Rational) -> Result<u64, MediaError> {
    let us = map_frames_with_rounding(
        frames,
        fps,
        Rational::new(1_000_000, 1).expect("positive rate"),
        FrameRounding::Ceil,
    )
    .map_err(|e| MediaError::Backend(e.to_string()))?;
    u64::try_from(us.0).map_err(|e| MediaError::Backend(e.to_string()))
}
fn source_frame(
    micros: u64,
    fps: Rational,
    rounding: FrameRounding,
) -> Result<TimeCode, MediaError> {
    let time = TimeCode(i64::try_from(micros).map_err(|e| MediaError::Backend(e.to_string()))?);
    map_frames_with_rounding(
        time,
        Rational::new(1_000_000, 1).expect("positive rate"),
        fps,
        rounding,
    )
    .map_err(|e| MediaError::Backend(e.to_string()))
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
            if clip.speed_percent != 100 {
                continue;
            }
            let Some(asset) = document.asset(clip.asset) else {
                continue;
            };
            // MO1 R8: stills carry no transcript — skipped like M23's
            // speeded clips, not mapped.
            if asset.kind == MediaKind::Image {
                continue;
            }
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
#[serde(deny_unknown_fields)]
struct StoredTranscript {
    content_sha256: String,
    identity: Identity,
    source_fps: Rational,
    source_frames: TimeCode,
    language: Option<String>,
    recognition: Recognition,
}
impl StoredTranscript {
    fn into_asset(self, asset: AssetId) -> Result<AssetTranscript, MediaError> {
        let words = self
            .recognition
            .words
            .into_iter()
            .map(|word| {
                let source_start =
                    source_frame(word.start_us, self.source_fps, FrameRounding::Floor)?;
                let source_end = source_frame(word.end_us, self.source_fps, FrameRounding::Ceil)?
                    .min(self.source_frames);
                Ok(TranscriptWord {
                    text: word.text,
                    source_start,
                    source_end,
                    speaker: None,
                })
            })
            .collect::<Result<Vec<_>, MediaError>>()?;
        Ok(AssetTranscript {
            asset,
            content_sha256: self.content_sha256,
            source_fps: self.source_fps,
            words,
        })
    }
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
    fn path_for(
        &self,
        content: &str,
        identity: &Identity,
        fps: Rational,
        frames: TimeCode,
        language: Option<&str>,
    ) -> PathBuf {
        let language_key = crate::sha256::sha256_bytes(format!("{language:?}").as_bytes());
        self.cache.path_for(
            content,
            &format!(
                "-{}-{}-{}-{}-{language_key}",
                identity.cache_key(),
                fps.numerator(),
                fps.denominator(),
                frames.0
            ),
        )
    }
    fn load(
        &self,
        content: &str,
        identity: &Identity,
        fps: Rational,
        frames: TimeCode,
        language: Option<&str>,
    ) -> Result<Option<StoredTranscript>, MediaError> {
        self.cache.load(
            &self.path_for(content, identity, fps, frames, language),
            |stored: StoredTranscript| {
                (stored.content_sha256 == content
                    && stored.identity == *identity
                    && stored.source_fps == fps
                    && stored.source_frames == frames
                    && stored.language.as_deref() == language)
                    .then_some(stored)
            },
        )
    }
    fn save_completed(
        &self,
        result: Result<StoredTranscript, MediaError>,
        cancel: &ExportCancellation,
    ) -> Result<StoredTranscript, MediaError> {
        let stored = result?;
        self.save(&stored, cancel)?;
        Ok(stored)
    }
    fn save(
        &self,
        stored: &StoredTranscript,
        cancel: &ExportCancellation,
    ) -> Result<(), MediaError> {
        cancelled(cancel)?;
        self.cache.save(
            &self.path_for(
                &stored.content_sha256,
                &stored.identity,
                stored.source_fps,
                stored.source_frames,
                stored.language.as_deref(),
            ),
            stored,
        )
    }
}
/// Explicit preparation uses the same pinned weights and installer as asset recognition.
/// # Errors
/// Returns typed provisioning, verification or cancellation errors as media errors.
pub fn provision_voice_model(
    data_dir: &Path,
    cancellation: &ExportCancellation,
) -> Result<PathBuf, MediaError> {
    recognizer::provision(&ENGLISH_MODEL, data_dir, cancellation, |_, _| {})
        .map(|root| root.join(ENGLISH_MODEL.files[0].name))
        .map_err(Into::into)
}
fn cancelled(cancel: &ExportCancellation) -> Result<(), MediaError> {
    recognizer::cancelled(cancel).map_err(Into::into)
}

/// Bounded, CPU-only note inference. Missing or altered models require explicit preparation.
pub(crate) fn transcribe_voice_wav(
    data_dir: &Path,
    content_sha256: &str,
    audio: &[u8],
    cancellation: &ExportCancellation,
) -> Result<crate::voice_note::VoiceTranscript, MediaError> {
    use crate::voice_note::{VoiceTranscript, VoiceWord};
    let owned_data_dir = crate::data_dir::ensure_root(data_dir)?;
    let data_dir = owned_data_dir.as_path();
    cancelled(cancellation)?;
    let (samples, duration) = crate::voice_note::voice_pcm(audio)?;
    if samples.iter().all(|sample| sample.abs() < 0.001) {
        return Ok(VoiceTranscript::NoSpeech);
    }
    let store = TranscriptStore::new(cache_root(data_dir, "voice-transcripts", CACHE_VERSION));
    let fps = Rational::new(1_000_000, 1).expect("positive rate");
    let frames = TimeCode(i64::try_from(duration).map_err(|e| MediaError::Backend(e.to_string()))?);
    let identity = Identity::english();
    let cached = store.load(content_sha256, &identity, fps, frames, Some("en"))?;
    let stored = if let Some(cached) = cached {
        cached
    } else {
        let root = data_dir.join("models").join(ENGLISH_MODEL.id);
        let path = root.join(ENGLISH_MODEL.files[0].name);
        match recognizer::model::verify_at(
            data_dir,
            &ENGLISH_MODEL.files[0],
            &path,
            &root.join("verified"),
            cancellation,
        ) {
            Ok(_) => {}
            Err(recognizer::Error::Model(
                recognizer::ModelError::Missing
                | recognizer::ModelError::WrongLength { .. }
                | recognizer::ModelError::WrongHash { .. }
                | recognizer::ModelError::Changed,
            )) => return Ok(VoiceTranscript::AwaitingModel),
            Err(error) => return Err(error.into()),
        }
        let result =
            recognizer::queue::samples(data_dir, &root, samples, cancellation.clone(), identity)
                .map_err(MediaError::from)?;
        StoredTranscript {
            content_sha256: content_sha256.into(),
            identity: result.identity,
            source_fps: fps,
            source_frames: frames,
            language: Some("en".into()),
            recognition: result.recognition,
        }
    };
    let recognition = &stored.recognition;
    if recognition.words.len() > 4096
        || recognition
            .words
            .iter()
            .map(|w| w.text.len())
            .sum::<usize>()
            > crate::voice_note::MAX_VOICE_TEXT_BYTES
        || recognition
            .words
            .iter()
            .any(|w| w.start_us >= w.end_us || w.end_us > duration)
        || recognition
            .words
            .windows(2)
            .any(|w| w[0].start_us > w[1].start_us || w[0].end_us > w[1].end_us)
    {
        return Err(MediaError::Backend(
            "recognizer returned invalid voice timing or text bounds".into(),
        ));
    }
    store.save(&stored, cancellation)?;
    cancelled(cancellation)?;
    let words = stored
        .recognition
        .words
        .into_iter()
        .map(|word| VoiceWord {
            text: word.text,
            start_micros: word.start_us,
            end_micros: word.end_us,
        })
        .collect::<Vec<_>>();
    Ok(if words.is_empty() {
        VoiceTranscript::NoSpeech
    } else {
        VoiceTranscript::Ready(words)
    })
}

#[cfg(test)]
mod tests {
    use kinewright_core::{Clip, ClipContent, ClipId, Track, TrackId, TrackKind};

    use super::*;
    use crate::test_support::TempDirectory;
    use std::fs;

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

    fn stored(hash: &str) -> StoredTranscript {
        StoredTranscript {
            content_sha256: hash.into(),
            identity: Identity::english(),
            source_fps: Rational::new(24, 1).unwrap(),
            source_frames: TimeCode(240),
            language: Some("en".into()),
            recognition: Recognition {
                timing_source: recognizer::TimingSource::TokenDurationsLagged,
                words: vec![recognizer::Word {
                    text: "Um,".into(),
                    start_us: 1_001_000,
                    end_us: 1_243_000,
                }],
            },
        }
    }
    #[test]
    fn dq1_exact_cache_round_trip_and_recipe_misses() {
        let data = TempDirectory::new("dq1-cache");
        let store = TranscriptStore::new(data.root().into());
        let entry = stored(&crate::sha256::sha256_bytes(b"same audio"));
        store.save(&entry, &ExportCancellation::default()).unwrap();
        let loaded = store
            .load(
                &entry.content_sha256,
                &entry.identity,
                entry.source_fps,
                entry.source_frames,
                Some("en"),
            )
            .unwrap()
            .unwrap();
        assert_eq!(
            loaded.recognition, entry.recognition,
            "microseconds and timing source survive cache"
        );
        let asset = loaded.into_asset(AssetId(99)).unwrap();
        assert_eq!(asset.asset, AssetId(99));
        assert_eq!(
            (asset.words[0].source_start, asset.words[0].source_end),
            (TimeCode(24), TimeCode(30))
        );
        let mutations = recognizer::tests::identity_mutations();
        for mutate in mutations {
            // Each lookup represents a separate request's recognizer settings.
            let mut identity = entry.identity.clone();
            mutate(&mut identity);
            assert!(
                store
                    .load(
                        &entry.content_sha256,
                        &identity,
                        entry.source_fps,
                        entry.source_frames,
                        Some("en")
                    )
                    .unwrap()
                    .is_none()
            );
        }
        for (hash, fps, frames, language) in [
            ("other", entry.source_fps, entry.source_frames, Some("en")),
            (
                &entry.content_sha256,
                Rational::new(30, 1).unwrap(),
                entry.source_frames,
                Some("en"),
            ),
            (
                &entry.content_sha256,
                entry.source_fps,
                TimeCode(120),
                Some("en"),
            ),
            (
                &entry.content_sha256,
                entry.source_fps,
                entry.source_frames,
                Some("fr"),
            ),
            (
                &entry.content_sha256,
                entry.source_fps,
                entry.source_frames,
                None,
            ),
        ] {
            assert!(
                store
                    .load(hash, &entry.identity, fps, frames, language)
                    .unwrap()
                    .is_none()
            );
        }
        fs::write(
            store.path_for(
                &entry.content_sha256,
                &entry.identity,
                entry.source_fps,
                entry.source_frames,
                Some("en"),
            ),
            b"not JSON",
        )
        .unwrap();
        assert!(
            store
                .load(
                    &entry.content_sha256,
                    &entry.identity,
                    entry.source_fps,
                    entry.source_frames,
                    Some("en")
                )
                .unwrap()
                .is_none()
        );
    }
    #[test]
    fn dq1_cancelled_recognition_leaves_no_cache_entry() {
        let data = TempDirectory::new("dq1-cancel-cache");
        let store = TranscriptStore::new(data.root().into());
        let entry = stored("cancelled");
        let cancel = ExportCancellation::default();
        let result = recognizer::tests::interrupted_recognition(&cancel).map(|recognition| {
            StoredTranscript {
                recognition,
                ..entry
            }
        });
        assert!(matches!(
            store.save_completed(result.map_err(Into::into), &cancel),
            Err(MediaError::Cancelled)
        ));
        let entry = stored("cancelled");
        assert!(
            store
                .load(
                    &entry.content_sha256,
                    &entry.identity,
                    entry.source_fps,
                    entry.source_frames,
                    Some("en")
                )
                .unwrap()
                .is_none()
        );
        assert!(!data.root().join("cancelled.json").exists());
        assert!(fs::read_dir(data.root()).unwrap().next().is_none());
    }
    #[test]
    fn c6b_inference_admitted_after_cancellation_aborts_on_revocation() {
        let origin = kinewright_core::MediaSourceOrigin::new_project();
        let grant = origin.legacy_media().unwrap();
        let registry = CancellationRegistry::default();
        let _authority = crate::locator::LocatorAuthority::enter(&grant);
        registry.cancel_all();
        let token = registry
            .start(Path::new("admitted-after-sweep.wav"))
            .unwrap();
        let worker_token = token.clone();
        let worker_grant = grant.clone();
        let (arrived, ready) = crossbeam_channel::bounded(0);
        let (release, released) = crossbeam_channel::bounded(0);
        let worker = std::thread::spawn(move || {
            let _access = crate::locator::LocatorAccess::enter(&worker_grant).unwrap();
            assert!(cancelled(&worker_token).is_ok());
            arrived.send(()).unwrap();
            released.recv().unwrap();
            cancelled(&worker_token).is_err()
        });
        ready
            .recv_timeout(std::time::Duration::from_secs(5))
            .unwrap();
        origin.revoke_legacy_media();
        assert!(
            !token.is_cancelled(),
            "the first sweep could not see this later token"
        );
        release.send(()).unwrap();
        assert!(
            worker.join().unwrap(),
            "C6B_LATE_ADMISSION_ABORTS_ON_REVOCATION"
        );
        assert!(grant.is_quiescent());
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
            source_fingerprint: kinewright_core::MediaSourceFingerprint::unknown(),
            color_description: kinewright_core::ColorDescription::default(),
            assumed_from: None,
        };
        let clip_duration = map_source_range_to_project(
            TimeCode(10)..TimeCode(40),
            source_fps,
            Rational::new(30, 1).unwrap(),
        )
        .unwrap();
        Document {
            investigator: None,
            catalog: kinewright_core::MediaCatalog::default(),
            audio_mix: kinewright_core::AudioMix::default(),
            color_context: kinewright_core::ColorContext::default(),
            lut_assets: Vec::new(),
            tracks: vec![Track {
                id: TrackId(7),
                kind: TrackKind::Video,
                sync_lock: true,
                clips: vec![Clip {
                    enabled: true,
                    enabled_curve: None,
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
                    audio_gain_curve: None,
                    blend_mode: kinewright_core::BlendMode::Normal,
                }],
            }],
            media_pool: vec![asset],
            markers: Vec::new(),
            fps: Rational::new(30, 1).unwrap(),
            resolution: (320, 180),
            duration: TimeCode(100 + clip_duration.0),
        }
    }

    /// MO1 R8: the transcript mapping skips Image clips — both a Media clip
    /// over a still (hand-built; the R9 ops refuse it) and the real Freeze
    /// shape — even when a transcript is offered for the asset.
    #[test]
    fn timeline_words_skip_image_clips() {
        let mut document = fixture_document(Rational::new(24, 1).unwrap());
        document.media_pool[0].kind = MediaKind::Image;
        let transcript = std::sync::Arc::new(fixture_transcript(
            AssetId(1),
            "fixture",
            Rational::new(24, 1).unwrap(),
        ));

        for content in [
            ClipContent::Media,
            ClipContent::Freeze(kinewright_core::FreezeFrame {
                source_frame: TimeCode(10),
            }),
        ] {
            document.tracks[0].clips[0].content = content.clone();
            assert!(
                map_timeline_words(&document, None, |_| Some(std::sync::Arc::clone(
                    &transcript
                )))
                .unwrap()
                .is_empty(),
                "words must skip {content:?} over a still"
            );
        }
    }
}
