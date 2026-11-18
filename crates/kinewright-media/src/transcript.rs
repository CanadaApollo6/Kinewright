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
    derived_cache::{
        CacheDirectory, CancellationRegistry, JsonCache, StatusReporter, cache_root, spawn_worker,
    },
    sha256::sha256_file,
};

// Zero-start decode retains codec preroll; older word results must be recomputed.
const CACHE_VERSION: u32 = 6;
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

mod requests;
use requests::{Recipe, Requests};

// A status key, never a locator. Owned jobs always carry their certified descriptor.
pub(crate) fn owned_dialogue_asset(
    asset: &MediaAsset,
    stream: u32,
) -> Result<MediaAsset, MediaError> {
    let sha256 = asset
        .source_fingerprint
        .content_sha256
        .as_deref()
        .ok_or(MediaError::MediaAdmissionRequired)?;
    let bytes = asset
        .source_fingerprint
        .byte_len
        .ok_or(MediaError::MediaAdmissionRequired)?;
    let mut bound = asset.clone();
    bound.path = PathBuf::from(format!("owned-dialogue/{sha256}/{bytes}/{stream}"));
    Ok(bound)
}

pub(crate) struct TranscriptService {
    jobs: crate::worker::OwnedWorker<TranscriptWork>,
    requests: Requests,
    profiles: Requests,
    root: CacheDirectory,
    #[cfg(any(test, feature = "test-util"))]
    inference_gate: InferenceGate,
}
enum TranscriptWork {
    Locator(crate::locator::LocatorJob<TranscriptJob>),
    Owned(TranscriptJob),
}
struct TranscriptJob {
    source: Option<crate::owned_io::CertifiedSource>,
    profile_only: bool,
    asset: MediaAsset,
    model: &'static recognizer::ModelSpec,
    recipe: Recipe,
    cancellation: ExportCancellation,
}
impl TranscriptJob {
    fn content_sha256(&self) -> Result<String, MediaError> {
        if let Some(source) = &self.source {
            source.check_read_authority()?;
            self.asset
                .source_fingerprint
                .content_sha256
                .as_ref()
                .ok_or(MediaError::MediaAdmissionRequired)
                .map(ToOwned::to_owned)
        } else {
            sha256_file(&self.asset.path)
        }
    }
}
impl Drop for TranscriptService {
    fn drop(&mut self) {
        self.requests.shutdown();
        self.profiles.shutdown();
    }
}
impl TranscriptService {
    pub(crate) fn owned_dialogue_cache(
        &self,
        asset: &MediaAsset,
        stream: u32,
        source: crate::owned_io::CertifiedSource,
        data_dir: &Path,
    ) -> Result<crate::dialogue_cache::DialogueCacheRead, MediaError> {
        use crate::{
            dialogue_cache::DialogueCacheRead,
            sound_profile::{ProfileIdentity, SoundMeasurement, SoundProfileCache},
        };
        if source.audio_stream()? != stream {
            return Err(MediaError::OwnedMediaOperationUnavailable);
        }
        let key = ProfileIdentity::new(&asset.source_fingerprint, stream)
            .ok_or(MediaError::MediaAdmissionRequired)?;
        let asset = owned_dialogue_asset(asset, stream)?;
        let words = self.dialogue_words(&asset)?;
        let profile = SoundProfileCache::new(data_dir)?.load(&key)?;
        match (words, profile) {
            (Some(words), Some(SoundMeasurement::Measured { profile })) => {
                Ok(DialogueCacheRead::Ready {
                    recogniser_key: words.recogniser_key,
                    words: words.words,
                    profile,
                })
            }
            (_, Some(SoundMeasurement::NoVoiceInformation { reason })) => {
                Ok(DialogueCacheRead::Pending(TranscriptStatus::Failed(
                    format!("no voice information: {reason}"),
                )))
            }
            (words, _) => Ok(DialogueCacheRead::Pending(self.request_owned_dialogue(
                asset,
                source,
                words.is_some(),
            ))),
        }
    }
    fn request_owned_dialogue(
        &self,
        asset: MediaAsset,
        source: crate::owned_io::CertifiedSource,
        profile_only: bool,
    ) -> TranscriptStatus {
        let requests = if profile_only {
            &self.profiles
        } else {
            &self.requests
        };
        let recipe = Recipe {
            identity: Identity::english(),
            language: None,
        };
        let admitted = if profile_only {
            requests.invalidate_finished(&asset.path);
            requests
                .admit(&asset.path, &recipe)
                .map(|cancel| (recipe, cancel))
        } else {
            requests.admit_cached_miss(&asset.path)
        };
        let path = asset.path.clone();
        if let Some((recipe, cancellation)) = admitted {
            self.enqueue(
                TranscriptJob {
                    source: Some(source),
                    profile_only,
                    asset,
                    model: &ENGLISH_MODEL,
                    recipe,
                    cancellation,
                },
                None,
            );
        }
        match requests.status(&path) {
            TranscriptStatus::Ready(_) | TranscriptStatus::NoSpeech => {
                TranscriptStatus::Transcribing {
                    progress_percent: 100,
                }
            }
            status => status,
        }
    }
    pub(crate) fn new(data_dir: PathBuf) -> Result<Self, MediaError> {
        let data_dir = crate::data_dir::ensure_root(data_dir)?;
        let (jobs, jobs_rx) = unbounded();
        let requests = Requests::new();
        let root = cache_root(&data_dir, "transcripts", CACHE_VERSION)?;
        let mut worker = TranscriptWorker::new(data_dir, root.clone(), requests.clone());
        let profiles = worker.profiles.clone();
        #[cfg(any(test, feature = "test-util"))]
        let inference_gate = Arc::clone(&worker.inference_gate);
        let jobs = spawn_worker(
            "kinewright-transcript",
            "transcript",
            (jobs, jobs_rx),
            move |job| match job {
                TranscriptWork::Locator(job) => {
                    worker.dispatch(job, TranscriptWorker::transcribe_asset)
                }
                TranscriptWork::Owned(job) => worker.handle(&job, |worker, job| {
                    job.source
                        .as_ref()
                        .ok_or(MediaError::MediaAdmissionRequired)?
                        .check_read_authority()?;
                    worker.transcribe_asset(job)
                }),
            },
        )?;
        Ok(Self {
            jobs,
            requests,
            profiles,
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
            self.requests.no_audio(&asset.path);
            return;
        }
        let model = match recognizer::model_for_language(language.as_deref()) {
            Ok(model) => model,
            Err(error) => {
                self.requests.reject(&asset.path, &error);
                return;
            }
        };
        let recipe = Recipe {
            identity: Identity::for_model(model),
            language,
        };
        let Some(cancellation) = self.requests.admit(&asset.path, &recipe) else {
            return;
        };
        self.enqueue(
            TranscriptJob {
                source: None,
                profile_only: false,
                asset,
                model,
                recipe,
                cancellation,
            },
            Some(grant),
        );
    }
    fn enqueue(&self, job: TranscriptJob, grant: Option<kinewright_core::LegacyMediaGrant>) {
        let requests = if job.profile_only {
            &self.profiles
        } else {
            &self.requests
        };
        let path = job.asset.path.clone();
        let cancellation = job.cancellation.clone();
        if self
            .jobs
            .send(match grant {
                Some(grant) => TranscriptWork::Locator(crate::locator::LocatorJob::new(grant, job)),
                None => TranscriptWork::Owned(job),
            })
            .is_err()
        {
            requests.update(
                &path,
                &cancellation,
                TranscriptStatus::Failed("transcript worker stopped".into()),
            );
            requests.finish(&path, &cancellation);
        }
    }
    pub(crate) fn request_missing_words(
        &self,
        asset: MediaAsset,
        grant: kinewright_core::LegacyMediaGrant,
    ) -> TranscriptStatus {
        let _authority = crate::locator::LocatorAuthority::enter(&grant);
        let path = asset.path.clone();
        if let Some((recipe, cancellation)) = self.requests.admit_cached_miss(&path) {
            match recognizer::model_for_language(recipe.language.as_deref()) {
                Ok(model) => self.enqueue(
                    TranscriptJob {
                        source: None,
                        profile_only: false,
                        asset,
                        model,
                        recipe,
                        cancellation,
                    },
                    Some(grant),
                ),
                Err(error) => {
                    self.requests.update(
                        &path,
                        &cancellation,
                        TranscriptStatus::Failed(error.to_string()),
                    );
                    self.requests.finish(&path, &cancellation);
                }
            }
        }
        match self.requests.status(&path) {
            TranscriptStatus::Ready(_) | TranscriptStatus::NoSpeech => {
                TranscriptStatus::Transcribing {
                    progress_percent: 100,
                }
            }
            status => status,
        }
    }
    pub(crate) fn status(&self, path: &Path) -> TranscriptStatus {
        self.requests.status(path)
    }
    pub(crate) fn retire(&self, authority: kinewright_core::LegacyMediaIdentity) {
        self.requests.retire(authority);
        self.profiles.retire(authority);
    }
    pub(crate) fn cancel_all(&self) {
        self.requests.cancel_all();
        self.profiles.cancel_all();
    }
    pub(crate) fn cancel(&self, path: &Path) -> bool {
        self.profiles.cancel(path) | self.requests.cancel(path)
    }
    pub(crate) fn register(&self, path: &Path, transcript: AssetTranscript) {
        self.requests.register(path, transcript);
    }
    pub(crate) fn dialogue_words(
        &self,
        asset: &MediaAsset,
    ) -> Result<Option<crate::dialogue_cache::WordEvidence>, MediaError> {
        use kinewright_core::{ExactMicros, dialogue::Word};
        let recipe = match self.requests.evidence_recipe(&asset.path) {
            Some(Ok(authored)) => {
                if authored.content_sha256
                    != asset
                        .source_fingerprint
                        .content_sha256
                        .as_deref()
                        .unwrap_or("")
                    || authored.source_fps != asset.fps
                {
                    return Err(MediaError::Backend(
                        "authored transcript identity changed".into(),
                    ));
                }
                let words = authored
                    .words
                    .iter()
                    .map(|word| {
                        Ok(Word {
                            text: word.text.clone(),
                            start_micros: ExactMicros::frames(word.source_start, asset.fps)
                                .map_err(|e| MediaError::Backend(e.to_string()))?
                                .floor(),
                            end_micros: ExactMicros::frames(word.source_end, asset.fps)
                                .map_err(|e| MediaError::Backend(e.to_string()))?
                                .ceil(),
                        })
                    })
                    .collect::<Result<_, MediaError>>()?;
                return Ok(Some(crate::dialogue_cache::WordEvidence {
                    recogniser_key: "authored-source-frames-v1".into(),
                    words,
                }));
            }
            Some(Err(recipe)) => recipe,
            None => {
                if matches!(
                    self.requests.status(&asset.path),
                    TranscriptStatus::Failed(_) | TranscriptStatus::Cancelled
                ) {
                    return Ok(None);
                }
                Recipe {
                    identity: Identity::english(),
                    language: None,
                }
            }
        };
        let Some(content) = asset.source_fingerprint.content_sha256.as_deref() else {
            return Ok(None);
        };
        Ok(TranscriptStore::new(self.root.clone())
            .load(
                content,
                &recipe.identity,
                asset.fps,
                asset.duration,
                recipe.language.as_deref(),
            )?
            .map(|stored| crate::dialogue_cache::WordEvidence {
                recogniser_key: stored.identity.cache_key(),
                words: stored
                    .recognition
                    .words
                    .into_iter()
                    .map(|word| Word {
                        text: word.text,
                        start_micros: word.start_us,
                        end_micros: word.end_us,
                    })
                    .collect(),
            }))
    }
    pub(crate) fn request_profile(
        &self,
        asset: MediaAsset,
        grant: kinewright_core::LegacyMediaGrant,
    ) -> TranscriptStatus {
        let _authority = crate::locator::LocatorAuthority::enter(&grant);
        self.profiles.invalidate_finished(&asset.path);
        if matches!(
            self.profiles.status(&asset.path),
            TranscriptStatus::NotRequested
        ) {
            let recipe = Recipe {
                identity: Identity::english(),
                language: None,
            };
            if let Some(cancellation) = self.profiles.admit(&asset.path, &recipe) {
                let path = asset.path.clone();
                self.enqueue(
                    TranscriptJob {
                        source: None,
                        profile_only: true,
                        asset,
                        model: &ENGLISH_MODEL,
                        recipe,
                        cancellation,
                    },
                    Some(grant),
                );
                return match self.profiles.status(&path) {
                    TranscriptStatus::NoSpeech => TranscriptStatus::Transcribing {
                        progress_percent: 100,
                    },
                    status => status,
                };
            }
        }
        self.profiles.status(&asset.path)
    }
    pub(crate) fn cache_stats(&self) -> Result<crate::derived_cache::CacheStats, MediaError> {
        self.root.inventory()
    }
    pub(crate) fn cache_root(&self) -> &Path {
        self.root.path()
    }
    pub(crate) fn clear_cache(&self) -> Result<crate::derived_cache::CacheStats, MediaError> {
        self.root.clear()
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
}
struct TranscriptWorker {
    data_dir: PathBuf,
    requests: Requests,
    profiles: Requests,
    store: TranscriptStore,
    #[cfg(any(test, feature = "test-util"))]
    inference_gate: InferenceGate,
}
impl TranscriptWorker {
    fn new(data_dir: PathBuf, root: CacheDirectory, requests: Requests) -> Self {
        Self {
            store: TranscriptStore::new(root),
            profiles: Requests::new(),
            data_dir,
            requests,
            #[cfg(any(test, feature = "test-util"))]
            inference_gate: Arc::default(),
        }
    }
    fn dispatch(
        &mut self,
        job: crate::locator::LocatorJob<TranscriptJob>,
        transcribe: impl FnOnce(&mut Self, &TranscriptJob) -> Result<(), MediaError>,
    ) -> bool {
        job.run(
            self,
            |worker, job| worker.handle(&job, transcribe),
            |worker, job, error| {
                worker.update(&job, TranscriptStatus::Failed(error.to_string()));
                worker
                    .job_requests(&job)
                    .finish(&job.asset.path, &job.cancellation);
                true
            },
        )
    }
    fn handle(
        &mut self,
        job: &TranscriptJob,
        transcribe: impl FnOnce(&mut Self, &TranscriptJob) -> Result<(), MediaError>,
    ) -> bool {
        let result =
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| transcribe(self, job)))
                .unwrap_or_else(|_| Err(recognizer::Error::WorkerPanicked.into()));
        if let Err(error) = result {
            self.update(
                job,
                if error == MediaError::Cancelled {
                    TranscriptStatus::Cancelled
                } else {
                    TranscriptStatus::Failed(error.to_string())
                },
            );
        }
        self.job_requests(job)
            .finish(&job.asset.path, &job.cancellation);
        true
    }
    fn transcribe_asset(&mut self, job: &TranscriptJob) -> Result<(), MediaError> {
        let asset = &job.asset;
        let cancellation = &job.cancellation;
        let language = job.recipe.language.as_deref();
        let identity = &job.recipe.identity;
        cancelled(cancellation)?;
        self.update(job, TranscriptStatus::Hashing);
        let content_sha256 = job.content_sha256()?;
        cancelled(cancellation)?;
        let profile_target = self.profile_target(asset, &content_sha256, job.source.as_ref())?;
        if job.profile_only {
            self.fill_profile(job, profile_target)?;
            self.update(job, TranscriptStatus::NoSpeech);
            return Ok(());
        }
        if let Some(cached) = self.store.load(
            &content_sha256,
            identity,
            asset.fps,
            asset.duration,
            language,
        )? {
            self.fill_profile(job, profile_target)?;
            cancelled(cancellation)?;
            self.finish(job, cached.into_asset(asset.id)?);
            return Ok(());
        }
        #[cfg(any(test, feature = "test-util"))]
        self.hold_before_inference(asset, cancellation, job.source.as_ref())?;
        let model = job.model;
        let model_root = recognizer::provision(
            model,
            &self.data_dir,
            cancellation,
            |downloaded_bytes, total_bytes| {
                self.requests.update(
                    &asset.path,
                    cancellation,
                    TranscriptStatus::DownloadingModel {
                        model_name: model.name.into(),
                        downloaded_bytes,
                        total_bytes,
                    },
                );
            },
        )
        .map_err(MediaError::from)?;
        self.update(
            job,
            TranscriptStatus::Transcribing {
                progress_percent: 0,
            },
        );
        let requests = self.requests.clone();
        let asset_path = asset.path.clone();
        let request_cancel = cancellation.clone();
        let source_us = source_duration_us(asset.duration, asset.fps)?;
        // The queued loaded model can outlive this job while another request
        // waits, so it retains an owned copy of the frozen request identity.
        let recognized = recognizer::queue::file(
            &self.data_dir,
            &model_root,
            asset.path.clone(),
            asset.duration,
            asset.fps,
            cancellation.clone(),
            identity.clone(),
            profile_target,
            // Model work retains the certified descriptor until decoding finishes.
            job.source.clone(),
            move |processed| {
                let progress_percent =
                    u8::try_from(processed.saturating_mul(100) / source_us.max(1))
                        .unwrap_or(100)
                        .min(100);
                requests.update(
                    &asset_path,
                    &request_cancel,
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
        self.finish(job, stored.into_asset(asset.id)?);
        Ok(())
    }
    fn fill_profile(
        &self,
        job: &TranscriptJob,
        target: Option<crate::sound_profile::ProfileTarget>,
    ) -> Result<(), MediaError> {
        if let Some(target) = target {
            self.update(
                job,
                TranscriptStatus::Transcribing {
                    progress_percent: 0,
                },
            );
            let measured = crate::sound_profile::measure_file(
                &job.asset.path,
                &job.cancellation,
                job.source.as_ref(),
            )?;
            target.save(measured, &job.cancellation)?;
        }
        Ok(())
    }
    fn profile_target(
        &self,
        asset: &MediaAsset,
        content: &str,
        source: Option<&crate::owned_io::CertifiedSource>,
    ) -> Result<Option<crate::sound_profile::ProfileTarget>, MediaError> {
        use crate::sound_profile::{ProfileIdentity, ProfileTarget, SoundProfileCache};
        if !asset.source_fingerprint.is_verified() {
            return Ok(None);
        }
        if asset.source_fingerprint.content_sha256.as_deref() != Some(content)
            || asset.source_fingerprint.byte_len
                != Some(if let Some(source) = source {
                    source.identity().source_length
                } else {
                    std::fs::metadata(&asset.path)
                        .map_err(|e| MediaError::Backend(e.to_string()))?
                        .len()
                })
        {
            return Err(MediaError::Backend(
                "source content no longer matches its identity; re-import it before measurement"
                    .into(),
            ));
        }
        let stream = if let Some(source) = source {
            source.audio_stream()?
        } else {
            crate::sound_profile::audio_stream(&asset.path)?
        };
        let Some(identity) = ProfileIdentity::new(&asset.source_fingerprint, stream) else {
            return Ok(None);
        };
        let cache = SoundProfileCache::new(&self.data_dir)?;
        Ok(if cache.load(&identity)?.is_some() {
            None
        } else {
            Some(ProfileTarget { cache, identity })
        })
    }
    #[cfg(any(test, feature = "test-util"))]
    fn hold_before_inference(
        &self,
        asset: &MediaAsset,
        cancellation: &ExportCancellation,
        source: Option<&crate::owned_io::CertifiedSource>,
    ) -> Result<(), MediaError> {
        let gate = self.inference_gate.lock().expect("inference gate").take();
        if let Some((arrived, released)) = gate {
            let end =
                crate::clock::frame_to_samples(asset.duration, recognizer::SAMPLE_RATE, asset.fps);
            let mut decoder = crate::audio::AudioDecoder::open_with_source(
                &asset.path,
                recognizer::SAMPLE_RATE,
                1,
                0,
                end,
                source,
            )?;
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
    fn finish(&self, job: &TranscriptJob, transcript: AssetTranscript) {
        self.update(
            job,
            if transcript.words.is_empty() {
                TranscriptStatus::NoSpeech
            } else {
                TranscriptStatus::Ready(Arc::new(transcript))
            },
        );
    }
    fn update(&self, job: &TranscriptJob, status: TranscriptStatus) {
        self.job_requests(job)
            .update(&job.asset.path, &job.cancellation, status);
    }
    fn job_requests(&self, job: &TranscriptJob) -> &Requests {
        if job.profile_only {
            &self.profiles
        } else {
            &self.requests
        }
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
    fn new(root: CacheDirectory) -> Self {
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
    let store = TranscriptStore::new(cache_root(data_dir, "voice-transcripts", CACHE_VERSION)?);
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

    #[test]
    fn dq_media_exact_cached_words_follow_the_requested_recogniser_recipe() {
        let root = TempDirectory::new_in(
            &crate::test_support::workspace_target_dir(),
            "dq-media-exact-words",
        );
        let service = TranscriptService::new(root.root().into()).unwrap();
        let mut job = worker_job(&service.requests, Path::new("exact.wav"), 1);
        job.asset.source_fingerprint = kinewright_core::MediaSourceFingerprint {
            content_sha256: Some("a".repeat(64)),
            byte_len: Some(10),
        };
        let store = TranscriptStore::new(service.root.clone());
        let mut stored = StoredTranscript {
            content_sha256: "a".repeat(64),
            identity: Identity::english(),
            source_fps: job.asset.fps,
            source_frames: job.asset.duration,
            language: None,
            recognition: Recognition {
                timing_source: recognizer::TimingSource::DecoderTokenEstimates,
                words: vec![recognizer::Word {
                    text: "exact".into(),
                    start_us: 12_345,
                    end_us: 26_789,
                }],
            },
        };
        store.save(&stored, &job.cancellation).unwrap();
        let words = service.dialogue_words(&job.asset).unwrap().unwrap();
        assert_eq!(
            words.words[0].start_micros, 12_345,
            "DQ_MEDIA_WORDS_NOT_FRAME_QUANTISED"
        );
        assert_eq!(words.words[0].end_micros, 26_789);
        assert_eq!(words.recogniser_key, stored.identity.cache_key());
        let recipe = Recipe {
            identity: Identity::for_model(&recognizer::PARAKEET_MODEL),
            language: Some("fr".into()),
        };
        let changed = service.requests.admit(&job.asset.path, &recipe).unwrap();
        assert!(
            service.dialogue_words(&job.asset).unwrap().is_none(),
            "DQ_MEDIA_RECOGNISER_KEY_MISS"
        );
        stored.identity = recipe.identity;
        stored.language = recipe.language;
        stored.recognition.words[0].text = "français".into();
        store.save(&stored, &changed).unwrap();
        let words = service.dialogue_words(&job.asset).unwrap().unwrap();
        assert_eq!(words.words[0].text, "français");
        assert_eq!(words.recogniser_key, stored.identity.cache_key());
    }

    fn worker_job(requests: &Requests, path: &Path, id: u64) -> TranscriptJob {
        let recipe = Recipe {
            identity: Identity::english(),
            language: None,
        };
        TranscriptJob {
            source: None,
            profile_only: false,
            asset: MediaAsset {
                remote_locator: None,
                id: AssetId(id),
                name: "fixture".into(),
                path: path.to_path_buf(),
                kind: MediaKind::Audio,
                duration: TimeCode(25),
                fps: Rational::new(25, 1).unwrap(),
                resolution: None,
                source_fingerprint: kinewright_core::MediaSourceFingerprint::unknown(),
                color_description: kinewright_core::ColorDescription::default(),
                assumed_from: None,
            },
            model: &ENGLISH_MODEL,
            cancellation: requests.admit(path, &recipe).unwrap(),
            recipe,
        }
    }
    fn empty_transcript(job: &TranscriptJob) -> AssetTranscript {
        AssetTranscript {
            asset: job.asset.id,
            content_sha256: "0".repeat(64),
            source_fps: job.asset.fps,
            words: vec![],
        }
    }
    #[test]
    fn dq1_empty_words_finish_as_no_speech() {
        let root = TempDirectory::new("dq1-empty-completion");
        let requests = Requests::new();
        let job = worker_job(&requests, Path::new("empty.wav"), 1);
        let worker = TranscriptWorker::new(
            root.root().into(),
            cache_root(root.root(), "transcripts", CACHE_VERSION).unwrap(),
            requests.clone(),
        );
        worker.finish(&job, empty_transcript(&job));
        assert!(
            matches!(requests.status(&job.asset.path), TranscriptStatus::NoSpeech),
            "DQ1_EMPTY_WORDS_ARE_NO_SPEECH"
        );
    }
    #[test]
    fn dq1_transcript_worker_panic_fails_its_job_and_serves_queued_successor() {
        let root = TempDirectory::new("dq1-transcript-panic");
        let grant = crate::locator::fixture_grant();
        let _authority = crate::locator::LocatorAuthority::enter(&grant);
        let requests = Requests::new();
        let first = worker_job(&requests, Path::new("panics.wav"), 1);
        let second = worker_job(&requests, Path::new("successor.wav"), 2);
        let first_path = first.asset.path.clone();
        let second_path = second.asset.path.clone();
        let first_token = first.cancellation.clone();
        let mut worker = TranscriptWorker::new(
            root.root().into(),
            cache_root(root.root(), "transcripts", CACHE_VERSION).unwrap(),
            requests.clone(),
        );
        let (arrived, arrival) = crossbeam_channel::bounded(0);
        let (release, released) = crossbeam_channel::bounded(0);
        let (done, completed) = crossbeam_channel::bounded(2);
        let jobs = crossbeam_channel::unbounded();
        let queue = spawn_worker("dq1-transcript-panic", "fixture", jobs, move |job| {
            let result = worker.dispatch(job, |worker, job| {
                if job.asset.id == AssetId(1) {
                    arrived.send(()).unwrap();
                    released.recv().unwrap();
                    std::panic::resume_unwind(Box::new("injected transcript panic"));
                }
                worker.finish(job, empty_transcript(job));
                Ok(())
            });
            done.send(()).unwrap();
            result
        })
        .unwrap();
        queue
            .send(crate::locator::LocatorJob::new(grant.clone(), first))
            .unwrap();
        arrival.recv().unwrap();
        queue
            .send(crate::locator::LocatorJob::new(grant, second))
            .unwrap();
        assert!(matches!(
            requests.status(&second_path),
            TranscriptStatus::Queued
        ));
        release.send(()).unwrap();
        for _ in 0..2 {
            completed
                .recv_timeout(std::time::Duration::from_secs(30))
                .expect("DQ1_TRANSCRIPT_PANIC_SUCCESSOR_REPLY");
        }
        assert!(
            matches!(requests.status(&first_path), TranscriptStatus::Failed(reason) if reason == recognizer::Error::WorkerPanicked.to_string()),
            "DQ1_TRANSCRIPT_PANIC_TYPED_FAILURE"
        );
        assert!(
            matches!(requests.status(&second_path), TranscriptStatus::NoSpeech),
            "DQ1_TRANSCRIPT_PANIC_SUCCESSOR_FINISHES"
        );
        assert!(!requests.cancel(&first_path));
        assert!(
            requests
                .admit(
                    &first_path,
                    &Recipe {
                        identity: Identity::english(),
                        language: None
                    }
                )
                .is_some(),
            "DQ1_PANICKED_TRANSCRIPT_CAN_RETRY"
        );
        requests.update(&first_path, &first_token, TranscriptStatus::NoSpeech);
        assert!(matches!(
            requests.status(&first_path),
            TranscriptStatus::Queued
        ));
        drop(queue);
    }
    #[test]
    fn dq1_unsupported_language_never_queues_or_downloads_and_can_retry() {
        let root = TempDirectory::new("dq1-language-refusal");
        let service = TranscriptService::new(root.root().into()).unwrap();
        let grant = crate::locator::fixture_grant();
        let _authority = crate::locator::LocatorAuthority::enter(&grant);
        let requests = Requests::new();
        let job = worker_job(&requests, Path::new("missing.wav"), 1);
        service.request(job.asset.clone(), Some("ja".into()), grant.clone());
        assert!(
            matches!(service.status(&job.asset.path), TranscriptStatus::Failed(reason) if reason == "language not supported: ja")
        );
        assert!(
            !root.path("models").exists(),
            "DQ1_UNSUPPORTED_LANGUAGE_NEVER_PROVISIONS"
        );
        let recipe = Recipe {
            identity: Identity::english(),
            language: None,
        };
        assert!(service.requests.admit(&job.asset.path, &recipe).is_some());
        service.register(&job.asset.path, empty_transcript(&job));
        service.request(job.asset.clone(), Some("ko".into()), grant);
        assert!(
            matches!(service.status(&job.asset.path), TranscriptStatus::Ready(_)),
            "DQ1_LANGUAGE_REFUSAL_PRESERVES_AUTHORED"
        );
    }
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
                timing_source: recognizer::TimingSource::DecoderTokenEstimates,
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
        let store = TranscriptStore::new(CacheDirectory::new(data.root(), Path::new("")).unwrap());
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
        let store = TranscriptStore::new(CacheDirectory::new(data.root(), Path::new("")).unwrap());
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
            remote_locator: None,
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
            imported_from: Vec::new(),
            catalog: kinewright_core::MediaCatalog::default(),
            audio_mix: kinewright_core::AudioMix::default(),
            color_context: kinewright_core::ColorContext::default(),
            lut_assets: Vec::new(),
            tracks: vec![Track {
                id: TrackId(7),
                kind: TrackKind::Video,
                name: None,
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

#[cfg(test)]
mod measurement_tests {
    use super::*;
    #[test]
    fn dq_media_transcript_hit_fills_only_the_missing_profile_in_its_job() {
        use crate::sound_profile::{ProfileIdentity, SoundMeasurement, SoundProfileCache};
        use crate::test_support::{GeneratedMedia, TempDirectory, wav_f32};
        crate::initialize_ffmpeg().unwrap();
        let audio = GeneratedMedia::from_bytes(
            "dq-media-transcript-profile",
            "wav",
            &wav_f32(&vec![0.125; 3200], 16_000, 1),
        );
        let data = TempDirectory::new("dq-media-transcript-profile-root");
        let content = sha256_file(audio.path()).unwrap();
        let asset = MediaAsset {
            remote_locator: None,
            id: AssetId(1),
            path: audio.path().into(),
            name: "measured".into(),
            duration: TimeCode(5),
            fps: Rational::new(25, 1).unwrap(),
            kind: MediaKind::Audio,
            resolution: None,
            source_fingerprint: kinewright_core::MediaSourceFingerprint {
                content_sha256: Some(content.clone()),
                byte_len: Some(std::fs::metadata(audio.path()).unwrap().len()),
            },
            color_description: kinewright_core::ColorDescription::default(),
            assumed_from: None,
        };
        let requests = Requests::new();
        let recipe = Recipe {
            identity: Identity::english(),
            language: None,
        };
        let cancellation = requests.admit(&asset.path, &recipe).unwrap();
        let job = TranscriptJob {
            source: None,
            profile_only: false,
            asset,
            model: &ENGLISH_MODEL,
            recipe,
            cancellation,
        };
        let mut worker = TranscriptWorker::new(
            data.root().into(),
            cache_root(data.root(), "transcripts", CACHE_VERSION).unwrap(),
            requests,
        );
        worker
            .store
            .save_completed(
                Ok(StoredTranscript {
                    content_sha256: content,
                    identity: Identity::english(),
                    source_fps: job.asset.fps,
                    source_frames: job.asset.duration,
                    language: None,
                    recognition: Recognition {
                        timing_source: recognizer::TimingSource::DecoderTokenEstimates,
                        words: Vec::new(),
                    },
                }),
                &job.cancellation,
            )
            .unwrap();
        let key = ProfileIdentity::new(&job.asset.source_fingerprint, 0).unwrap();
        let cache = SoundProfileCache::new(data.root()).unwrap();
        assert!(cache.load(&key).unwrap().is_none());
        // A regression into inference fails before provisioning, so even a
        // broken cache-hit path cannot download a model in this fast test.
        let (arrived, arrival) = crossbeam_channel::bounded(1);
        let (release, released) = crossbeam_channel::bounded(1);
        drop(release);
        *worker.inference_gate.lock().unwrap() = Some((arrived, released));
        worker.transcribe_asset(&job).unwrap();
        assert!(matches!(
            arrival.try_recv(),
            Err(crossbeam_channel::TryRecvError::Empty)
        ));
        let Some(SoundMeasurement::Measured { profile }) = cache.load(&key).unwrap() else {
            panic!("transcript job must also publish its profile")
        };
        assert_eq!(profile.duration_micros(), 200_000);
        assert!(
            profile
                .hops()
                .all(|hop| (hop.rms_dbfs_hundredths + 1806).abs() <= 1)
        );
        assert!(
            worker
                .profile_target(&job.asset, &sha256_file(audio.path()).unwrap(), None)
                .unwrap()
                .is_none(),
            "a model change cannot request level remeasurement"
        );
    }
}
