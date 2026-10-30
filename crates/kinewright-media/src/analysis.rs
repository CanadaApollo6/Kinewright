use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use crossbeam_channel::{Receiver, Sender, bounded};
use kinewright_core::{
    AssetId, MediaAsset, MediaError, MediaKind, Rational, RgbaImage, ThumbnailFrame, ThumbnailKey,
    TimeCode, VisualAssetResult, VisualRequestKind, WaveformData, WaveformPeak,
};
use serde::{Deserialize, Serialize};

use crate::{
    audio::decode_audio_peaks,
    decode::thumbnail,
    derived_cache::{
        CacheStats, ContentHashes, JsonCache, TempFileStyle, atomic_write, cache_path, cache_root,
        clear_cache_root, create_cache_dir, inventory_cache_root, read_cache, spawn_worker,
        trim_cache,
    },
};

const CACHE_VERSION: u32 = 1;
const JOB_CAPACITY: usize = 64;
const RESULT_CAPACITY: usize = 64;
const WAVEFORM_SAMPLE_RATE: u32 = 8_000;
pub const MAX_WAVEFORM_PEAKS: usize = 2_048;
pub const MAX_THUMBNAIL_FILES: usize = 128;
pub const MAX_THUMBNAIL_BYTES: u64 = 64 * 1024 * 1024;
const MAX_WAVEFORM_FILES: usize = 128;
const MAX_WAVEFORM_BYTES: u64 = 32 * 1024 * 1024;
const THUMBNAIL_MAGIC: &[u8; 8] = b"ORTH0001";

type VisualJob = crate::locator::LocatorJob<(Job, Sender<VisualAssetResult>)>;
type VisualInFlight =
    Arc<Mutex<HashMap<(kinewright_core::LegacyMediaIdentity, JobKey), Sender<VisualAssetResult>>>>;
type VisualReply = (Sender<VisualAssetResult>, Receiver<VisualAssetResult>);
type VisualAuthorities =
    HashMap<kinewright_core::LegacyMediaIdentity, (kinewright_core::LegacyMediaGrant, VisualReply)>;

pub(crate) struct VisualAssetService {
    jobs: Sender<VisualJob>,
    results: Mutex<VisualAuthorities>,
    in_flight: VisualInFlight,
    root: PathBuf,
}

impl VisualAssetService {
    pub(crate) fn new(data_dir: &Path) -> Result<Self, MediaError> {
        let (jobs, jobs_rx) = bounded(JOB_CAPACITY);
        let in_flight = Arc::new(Mutex::new(HashMap::new()));
        let worker_in_flight = Arc::clone(&in_flight);
        let root = cache_root(data_dir, "visual-assets", CACHE_VERSION);
        let mut worker = VisualAssetWorker::new(root.clone(), worker_in_flight);
        spawn_worker(
            "kinewright-visual-assets",
            "visual asset",
            jobs_rx,
            move |job| worker.handle_scoped(job),
        )?;
        Ok(Self {
            jobs,
            results: Mutex::new(HashMap::new()),
            in_flight,
            root,
        })
    }

    pub(crate) fn request_waveform(
        &self,
        asset: MediaAsset,
        request_generation: u64,
        grant: kinewright_core::LegacyMediaGrant,
    ) -> bool {
        if !matches!(asset.kind, MediaKind::Audio | MediaKind::AudioVideo) {
            return false;
        }
        self.request(
            grant,
            Job::Waveform {
                asset,
                request_generation,
            },
        )
    }

    pub(crate) fn request_thumbnail(
        &self,
        asset: MediaAsset,
        source_at: TimeCode,
        max_width: u32,
        request_generation: u64,
        grant: kinewright_core::LegacyMediaGrant,
    ) -> bool {
        if !matches!(asset.kind, MediaKind::Video | MediaKind::AudioVideo) {
            return false;
        }
        let source_at = TimeCode(
            source_at
                .0
                .clamp(0, asset.duration.0.saturating_sub(1).max(0)),
        );
        self.request(
            grant,
            Job::Thumbnail {
                asset,
                source_at,
                max_width: max_width.clamp(1, 512),
                request_generation,
            },
        )
    }

    pub(crate) fn results(
        &self,
        grant: &kinewright_core::LegacyMediaGrant,
    ) -> Receiver<VisualAssetResult> {
        self.result_channel(grant)
            .map_or_else(crossbeam_channel::never, |(_, receiver)| receiver)
    }
    fn result_channel(&self, grant: &kinewright_core::LegacyMediaGrant) -> Option<VisualReply> {
        let mut results = self.results.lock().ok()?;
        results.retain(|_, (authority, _)| authority.check().is_ok());
        if !results.contains_key(&grant.identity()) && results.len() >= 64 {
            return None;
        }
        Some(
            results
                .entry(grant.identity())
                .or_insert_with(|| (grant.clone(), bounded(RESULT_CAPACITY)))
                .1
                .clone(),
        )
    }

    pub(crate) fn retire(&self, authority: kinewright_core::LegacyMediaIdentity) {
        if let Ok(mut results) = self.results.lock() {
            results.remove(&authority);
        }
    }

    pub(crate) fn cache_stats(&self) -> Result<CacheStats, MediaError> {
        inventory_cache_root(&self.root)
    }

    pub(crate) fn clear_cache(&self) -> Result<CacheStats, MediaError> {
        clear_cache_root(&self.root)
    }

    fn request(&self, grant: kinewright_core::LegacyMediaGrant, job: Job) -> bool {
        let key = (grant.identity(), job.key());
        let Some((reply, _)) = self.result_channel(&grant) else {
            return false;
        };
        let Ok(mut in_flight) = self.in_flight.lock() else {
            return false;
        };
        if let Some(pending) = in_flight.get(&key) {
            return pending.same_channel(&reply);
        }
        if reply.len()
            + in_flight
                .iter()
                .filter(|((authority, _), _)| *authority == key.0)
                .count()
            >= RESULT_CAPACITY
        {
            return false;
        }
        in_flight.insert(key.clone(), reply.clone());
        if let Ok(()) = self
            .jobs
            .try_send(crate::locator::LocatorJob::new(grant, (job, reply)))
        {
            true
        } else {
            in_flight.remove(&key);
            false
        }
    }
}

enum Job {
    Waveform {
        asset: MediaAsset,
        request_generation: u64,
    },
    Thumbnail {
        asset: MediaAsset,
        source_at: TimeCode,
        max_width: u32,
        request_generation: u64,
    },
}

impl Job {
    fn key(&self) -> JobKey {
        match self {
            Self::Waveform {
                asset,
                request_generation,
            } => JobKey::Waveform {
                asset: asset.id,
                path: asset.path.clone(),
                request_generation: *request_generation,
            },
            Self::Thumbnail {
                asset,
                source_at,
                max_width,
                request_generation,
            } => JobKey::Thumbnail {
                key: ThumbnailKey {
                    asset: asset.id,
                    source_at: *source_at,
                    max_width: *max_width,
                },
                path: asset.path.clone(),
                request_generation: *request_generation,
            },
        }
    }
}

/// Dedup keys carry the asset's PATH as well as its id: ids are per-document,
/// so with several projects open the same id can name different files, and an
/// id-only key would hand one project's pixels to another's request.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum JobKey {
    Waveform {
        asset: AssetId,
        path: PathBuf,
        request_generation: u64,
    },
    Thumbnail {
        key: ThumbnailKey,
        path: PathBuf,
        request_generation: u64,
    },
}

struct VisualAssetWorker {
    root: PathBuf,
    in_flight: VisualInFlight,
    hashes: ContentHashes,
}

impl VisualAssetWorker {
    fn new(root: PathBuf, in_flight: VisualInFlight) -> Self {
        Self {
            root,
            in_flight,
            hashes: ContentHashes,
        }
    }

    fn handle_scoped(&mut self, job: VisualJob) -> bool {
        // Result backpressure from an inactive project must not stop shared work.
        let (key, result, reply) = job.run(
            self,
            |worker, (job, reply)| {
                (
                    (
                        crate::locator::capture()
                            .expect("visual job authority")
                            .identity(),
                        job.key(),
                    ),
                    worker.process(job),
                    reply,
                )
            },
            |_, (job, reply), error| {
                let key = (
                    crate::locator::capture()
                        .expect("visual job authority")
                        .identity(),
                    job.key(),
                );
                let result = match job {
                    Job::Waveform {
                        asset,
                        request_generation,
                    } => VisualAssetResult::Failed {
                        asset: asset.id,
                        path: asset.path,
                        request_generation,
                        request: VisualRequestKind::Waveform,
                        message: error.to_string(),
                    },
                    Job::Thumbnail {
                        asset,
                        source_at,
                        max_width,
                        request_generation,
                    } => VisualAssetResult::Failed {
                        asset: asset.id,
                        path: asset.path,
                        request_generation,
                        request: VisualRequestKind::Thumbnail(ThumbnailKey {
                            asset: asset.id,
                            source_at,
                            max_width,
                        }),
                        message: error.to_string(),
                    },
                };
                (key, result, reply)
            },
        );
        if let Ok(mut in_flight) = self.in_flight.lock() {
            let _ = reply.try_send(result);
            in_flight.remove(&key);
        }
        true
    }

    fn process(&mut self, job: Job) -> VisualAssetResult {
        match job {
            Job::Waveform {
                asset,
                request_generation,
            } => self
                .waveform(&asset, request_generation)
                .unwrap_or_else(|error| VisualAssetResult::Failed {
                    asset: asset.id,
                    request_generation,
                    path: asset.path.clone(),
                    request: VisualRequestKind::Waveform,
                    message: error.to_string(),
                }),
            Job::Thumbnail {
                asset,
                source_at,
                max_width,
                request_generation,
            } => {
                let key = ThumbnailKey {
                    asset: asset.id,
                    source_at,
                    max_width,
                };
                self.thumbnail(&asset, key, request_generation)
                    .unwrap_or_else(|error| VisualAssetResult::Failed {
                        asset: asset.id,
                        request_generation,
                        path: asset.path.clone(),
                        request: VisualRequestKind::Thumbnail(key),
                        message: error.to_string(),
                    })
            }
        }
    }

    fn waveform(
        &mut self,
        asset: &MediaAsset,
        request_generation: u64,
    ) -> Result<VisualAssetResult, MediaError> {
        let hash = self.content_hash(&asset.path)?;
        let store = WaveformStore::new(self.root.join("waveforms"));
        if let Some(mut cached) = store.load(&hash, asset.fps, asset.duration)? {
            cached.asset = asset.id;
            cached.path.clone_from(&asset.path);
            cached.request_generation = request_generation;
            return Ok(VisualAssetResult::Waveform(Arc::new(cached)));
        }
        let peaks = decode_audio_peaks(
            &asset.path,
            asset.fps,
            asset.duration,
            WAVEFORM_SAMPLE_RATE,
            MAX_WAVEFORM_PEAKS,
        )?
        .into_iter()
        .map(|(minimum, maximum)| WaveformPeak { minimum, maximum })
        .collect();
        let waveform = WaveformData {
            asset: asset.id,
            request_generation,
            path: asset.path.clone(),
            content_sha256: hash,
            source_fps: asset.fps,
            source_frames: asset.duration,
            peaks,
        };
        store.save(&waveform)?;
        Ok(VisualAssetResult::Waveform(Arc::new(waveform)))
    }

    fn thumbnail(
        &mut self,
        asset: &MediaAsset,
        key: ThumbnailKey,
        request_generation: u64,
    ) -> Result<VisualAssetResult, MediaError> {
        let hash = self.content_hash(&asset.path)?;
        let store = ThumbnailStore::new(self.root.join("thumbnails"));
        let image =
            if let Some(cached) = store.load(&hash, asset.fps, key.source_at, key.max_width)? {
                cached
            } else {
                let decoded = thumbnail(&asset.path, asset.fps, key.source_at, key.max_width)?;
                store.save(&hash, asset.fps, key.source_at, key.max_width, &decoded)?;
                decoded
            };
        Ok(VisualAssetResult::Thumbnail(ThumbnailFrame {
            key,
            request_generation,
            path: asset.path.clone(),
            image: Arc::new(image),
        }))
    }

    fn content_hash(&mut self, path: &Path) -> Result<String, MediaError> {
        self.hashes.get(path)
    }
}

struct WaveformStore {
    cache: JsonCache,
}

impl WaveformStore {
    fn new(root: PathBuf) -> Self {
        Self {
            cache: JsonCache::new(root, CACHE_VERSION, "waveform")
                .with_write_options("visual asset", TempFileStyle::ReplaceExtension),
        }
    }

    fn load(
        &self,
        hash: &str,
        source_fps: Rational,
        source_frames: TimeCode,
    ) -> Result<Option<WaveformData>, MediaError> {
        self.cache
            .load(&self.cache.path_for(hash, ""), |stored: StoredWaveform| {
                let waveform = stored.waveform;
                (waveform.content_sha256 == hash
                    && waveform.source_fps == source_fps
                    && waveform.source_frames == source_frames
                    && waveform.peaks.len() <= MAX_WAVEFORM_PEAKS)
                    .then_some(waveform)
            })
    }

    fn save(&self, waveform: &WaveformData) -> Result<(), MediaError> {
        self.cache.save(
            &self.cache.path_for(&waveform.content_sha256, ""),
            &StoredWaveform {
                waveform: waveform.clone(),
            },
        )?;
        trim_cache(self.cache.root(), MAX_WAVEFORM_FILES, MAX_WAVEFORM_BYTES)?;
        Ok(())
    }
}

#[derive(Serialize, Deserialize)]
struct StoredWaveform {
    waveform: WaveformData,
}

struct ThumbnailStore {
    root: PathBuf,
}

impl ThumbnailStore {
    fn new(root: PathBuf) -> Self {
        Self { root }
    }

    fn load(
        &self,
        hash: &str,
        fps: Rational,
        source_at: TimeCode,
        max_width: u32,
    ) -> Result<Option<RgbaImage>, MediaError> {
        let Some(bytes) = read_cache(&self.path_for(hash, fps, source_at, max_width), "thumbnail")?
        else {
            return Ok(None);
        };
        decode_thumbnail_cache(&bytes).map_or(Ok(None), |image| Ok(Some(image)))
    }

    fn save(
        &self,
        hash: &str,
        fps: Rational,
        source_at: TimeCode,
        max_width: u32,
        image: &RgbaImage,
    ) -> Result<(), MediaError> {
        create_cache_dir(&self.root, "thumbnail")?;
        let bytes = encode_thumbnail_cache(image)?;
        atomic_write(
            &self.path_for(hash, fps, source_at, max_width),
            &bytes,
            "visual asset",
            TempFileStyle::ReplaceExtension,
        )?;
        trim_cache(&self.root, MAX_THUMBNAIL_FILES, MAX_THUMBNAIL_BYTES)?;
        Ok(())
    }

    fn path_for(&self, hash: &str, fps: Rational, source_at: TimeCode, max_width: u32) -> PathBuf {
        cache_path(
            &self.root,
            hash,
            &format!(
                "-{}-{}-{}-{max_width}",
                fps.numerator(),
                fps.denominator(),
                source_at.0
            ),
            "rgba",
        )
    }
}

fn encode_thumbnail_cache(image: &RgbaImage) -> Result<Vec<u8>, MediaError> {
    let expected = usize::try_from(image.width)
        .unwrap_or(usize::MAX)
        .saturating_mul(usize::try_from(image.height).unwrap_or(usize::MAX))
        .saturating_mul(4);
    if image.pixels.len() != expected {
        return Err(MediaError::Backend(
            "thumbnail cache image has an invalid RGBA length".to_owned(),
        ));
    }
    let mut bytes = Vec::with_capacity(16_usize.saturating_add(expected));
    bytes.extend_from_slice(THUMBNAIL_MAGIC);
    bytes.extend_from_slice(&image.width.to_le_bytes());
    bytes.extend_from_slice(&image.height.to_le_bytes());
    bytes.extend_from_slice(&image.pixels);
    Ok(bytes)
}

fn decode_thumbnail_cache(bytes: &[u8]) -> Option<RgbaImage> {
    if bytes.len() < 16 || &bytes[..8] != THUMBNAIL_MAGIC {
        return None;
    }
    let width = u32::from_le_bytes(bytes[8..12].try_into().ok()?);
    let height = u32::from_le_bytes(bytes[12..16].try_into().ok()?);
    let expected = usize::try_from(width)
        .ok()?
        .checked_mul(usize::try_from(height).ok()?)?
        .checked_mul(4)?;
    let pixels = bytes.get(16..)?;
    if pixels.len() != expected {
        return None;
    }
    Some(RgbaImage {
        width,
        height,
        pixels: pixels.to_vec(),
    })
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;
    use crate::test_support::TempDirectory;

    fn waveform(hash: &str) -> WaveformData {
        WaveformData {
            asset: AssetId(1),
            request_generation: 0,
            path: PathBuf::from("visual-1.wav"),
            content_sha256: hash.to_owned(),
            source_fps: Rational::new(30, 1).unwrap(),
            source_frames: TimeCode(300),
            peaks: vec![
                WaveformPeak {
                    minimum: -12_000,
                    maximum: 18_000,
                },
                WaveformPeak {
                    minimum: -5_000,
                    maximum: 7_000,
                },
            ],
        }
    }

    fn asset(id: u64) -> MediaAsset {
        MediaAsset {
            id: AssetId(id),
            path: PathBuf::from(format!("visual-{id}.wav")),
            name: format!("Visual {id}"),
            duration: TimeCode(300),
            fps: Rational::new(30, 1).unwrap(),
            kind: MediaKind::Audio,
            resolution: None,
            source_fingerprint: kinewright_core::MediaSourceFingerprint::unknown(),
            color_description: kinewright_core::ColorDescription::default(),
            assumed_from: None,
        }
    }

    #[test]
    fn c6b_thumbnail_cache_misses_when_fps_changes() {
        let directory = TempDirectory::new("c6b-thumbnail-recipe");
        let store = ThumbnailStore::new(directory.root().into());
        let image = RgbaImage {
            width: 1,
            height: 1,
            pixels: vec![12, 34, 56, 255],
        };
        let hash = crate::sha256::sha256_bytes(b"the same source bytes");
        let fps = Rational::new(24, 1).unwrap();
        store.save(&hash, fps, TimeCode(24), 32, &image).unwrap();
        assert_eq!(
            store
                .load(&hash, fps, TimeCode(24), 32)
                .unwrap()
                .unwrap()
                .pixels,
            image.pixels
        );
        assert!(
            store
                .load(&hash, Rational::new(30, 1).unwrap(), TimeCode(24), 32)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn c6a_retire_visual_channel_releases_registry_and_preserves_other_project() {
        let directory = TempDirectory::new("c6a-visual-retire");
        let (jobs, receiver) = bounded(JOB_CAPACITY);
        let service = VisualAssetService {
            jobs,
            results: Mutex::default(),
            in_flight: Arc::default(),
            root: directory.root().into(),
        };
        let a = kinewright_core::MediaSourceOrigin::new_project()
            .legacy_media()
            .unwrap();
        let b = kinewright_core::MediaSourceOrigin::new_project()
            .legacy_media()
            .unwrap();
        let old = service.results(&a);
        let other = service.results(&b);
        assert!(service.request_waveform(asset(1), 1, a.clone()));
        let pending = receiver.recv().unwrap();
        service.retire(a.identity());
        assert_eq!(service.results.lock().unwrap().len(), 1);
        let fresh = service.results(&a);
        assert!(
            !service.request_waveform(asset(1), 1, a.clone()),
            "C6A_RETIRED_REPLY_REFUSES_DUPLICATE_UNTIL_COMPLETION"
        );
        let mut worker =
            VisualAssetWorker::new(directory.root().into(), Arc::clone(&service.in_flight));
        assert!(worker.handle_scoped(pending));
        assert!(
            old.try_recv().is_ok(),
            "accepted work keeps its original channel"
        );
        assert!(fresh.is_empty(), "focus gets a fresh channel");
        assert!(service.request_waveform(asset(1), 1, a));
        assert!(worker.handle_scoped(receiver.recv().unwrap()));
        assert!(
            fresh.try_recv().is_ok(),
            "C6A_FRESH_CHANNEL_RETRY_RECEIVES_REPLY"
        );
        assert!(service.request_waveform(asset(2), 1, b));
        assert!(worker.handle_scoped(receiver.recv().unwrap()));
        assert!(other.try_recv().is_ok());
    }

    #[test]
    fn c6b_full_inactive_visual_channel_preserves_active_progress() {
        let directory = TempDirectory::new("c6b-visual-backpressure");
        let (jobs, receiver) = bounded(JOB_CAPACITY);
        let service = VisualAssetService {
            jobs,
            results: Mutex::default(),
            in_flight: Arc::default(),
            root: directory.root().into(),
        };
        let a = kinewright_core::MediaSourceOrigin::new_project()
            .legacy_media()
            .unwrap();
        let b = kinewright_core::MediaSourceOrigin::new_project()
            .legacy_media()
            .unwrap();
        let a_results = service.results(&a);
        let b_results = service.results(&b);
        let mut worker =
            VisualAssetWorker::new(directory.root().into(), Arc::clone(&service.in_flight));
        for generation in 0..RESULT_CAPACITY as u64 {
            assert!(service.request_waveform(asset(1), generation, a.clone()));
            assert!(worker.handle_scoped(receiver.recv().unwrap()));
        }
        assert_eq!(a_results.len(), RESULT_CAPACITY);
        assert!(
            !service.request_waveform(asset(1), RESULT_CAPACITY as u64, a.clone()),
            "a full authority refuses before accepting work"
        );
        assert!(service.request_waveform(asset(2), 0, b));
        assert!(worker.handle_scoped(receiver.recv().unwrap()));
        assert!(matches!(
            b_results.try_recv().unwrap(),
            VisualAssetResult::Failed {
                asset: AssetId(2),
                ..
            }
        ));
        for _ in 0..RESULT_CAPACITY {
            a_results.try_recv().unwrap();
        }
        assert!(service.request_waveform(asset(1), RESULT_CAPACITY as u64, a));
        assert!(worker.handle_scoped(receiver.recv().unwrap()));
        assert!(
            a_results.try_recv().is_ok(),
            "A retries after draining without losing accepted results"
        );
    }

    #[test]
    fn c2_r4_equal_visual_jobs_and_results_are_partitioned_by_authority() {
        let (jobs, jobs_rx) = bounded(4);
        let service = VisualAssetService {
            jobs,
            results: Mutex::default(),
            in_flight: Arc::default(),
            root: PathBuf::new(),
        };
        let a = kinewright_core::MediaSourceOrigin::new_project()
            .legacy_media()
            .unwrap();
        let b = kinewright_core::MediaSourceOrigin::new_project()
            .legacy_media()
            .unwrap();
        let request = || Job::Waveform {
            asset: asset(1),
            request_generation: 1,
        };
        assert!(service.request(a.clone(), request()));
        assert!(service.request(b.clone(), request()));
        assert_eq!(
            jobs_rx.len(),
            2,
            "equal sources still run under both requesting authorities"
        );
        let a_results = service.results(&a);
        let b_results = service.results(&b);
        service
            .result_channel(&a)
            .unwrap()
            .0
            .send(VisualAssetResult::Failed {
                asset: AssetId(1),
                path: PathBuf::from("same.wav"),
                request_generation: 1,
                request: VisualRequestKind::Waveform,
                message: "A result".into(),
            })
            .unwrap();
        assert!(
            b_results.try_recv().is_err(),
            "B cannot consume A's retained result"
        );
        assert!(
            matches!(a_results.try_recv().unwrap(), VisualAssetResult::Failed { message, .. } if message == "A result")
        );
    }

    #[test]
    fn c2_r4_a_closed_authority_result_channel_keeps_the_shared_worker_running() {
        let origin = kinewright_core::MediaSourceOrigin::new_project();
        let grant = origin.legacy_media().unwrap();
        origin.revoke_legacy_media();
        let (reply, receiver) = bounded(1);
        drop(receiver);
        let mut worker = VisualAssetWorker::new(PathBuf::new(), Arc::default());
        let job = Job::Waveform {
            asset: asset(1),
            request_generation: 1,
        };
        assert!(
            worker.handle_scoped(crate::locator::LocatorJob::new(grant, (job, reply))),
            "one closed project's reply cannot terminate every other project's worker"
        );
    }

    #[test]
    fn a_full_job_queue_is_reported_without_leaking_in_flight_keys() {
        let (jobs, _jobs_rx) = bounded(1);
        let in_flight = Arc::new(Mutex::new(HashMap::new()));
        let service = VisualAssetService {
            jobs,
            results: Mutex::default(),
            in_flight: Arc::clone(&in_flight),
            root: PathBuf::from("visual-assets/v1"),
        };

        let grant = kinewright_core::MediaSourceOrigin::new_project()
            .legacy_media()
            .unwrap();
        assert!(service.request(
            grant.clone(),
            Job::Waveform {
                asset: asset(1),
                request_generation: 1,
            }
        ));
        assert!(!service.request(
            grant.clone(),
            Job::Waveform {
                asset: asset(2),
                request_generation: 1,
            }
        ));
        assert_eq!(in_flight.lock().unwrap().len(), 1);
        assert!(in_flight.lock().unwrap().contains_key(&(
            grant.identity(),
            JobKey::Waveform {
                asset: AssetId(1),
                path: asset(1).path,
                request_generation: 1,
            }
        )));
    }

    #[test]
    fn same_id_different_files_are_distinct_jobs() {
        let mut second = asset(1);
        second.path = PathBuf::from("other-project.wav");
        assert_ne!(
            Job::Waveform {
                asset: asset(1),
                request_generation: 1,
            }
            .key(),
            Job::Waveform {
                asset: second,
                request_generation: 1,
            }
            .key()
        );
    }

    #[test]
    fn waveform_cache_round_trips_and_rebinds_asset() {
        let directory = TempDirectory::new("waveform-cache");
        let store = WaveformStore::new(directory.root().to_path_buf());
        let source = waveform("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa");
        store.save(&source).unwrap();

        let mut loaded = store
            .load(
                &source.content_sha256,
                source.source_fps,
                source.source_frames,
            )
            .unwrap()
            .unwrap();
        loaded.asset = AssetId(99);

        assert_eq!(loaded.asset, AssetId(99));
        assert_eq!(loaded.peaks, source.peaks);
        assert_eq!(loaded.content_sha256, source.content_sha256);
    }

    #[test]
    fn waveform_cache_rejects_metadata_mismatch() {
        let directory = TempDirectory::new("waveform-mismatch");
        let store = WaveformStore::new(directory.root().to_path_buf());
        let source = waveform("bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb");
        store.save(&source).unwrap();

        assert!(
            store
                .load(
                    &source.content_sha256,
                    Rational::new(24, 1).unwrap(),
                    source.source_frames
                )
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn thumbnail_cache_round_trips_and_rejects_corruption() {
        let image = RgbaImage {
            width: 2,
            height: 1,
            pixels: vec![1, 2, 3, 255, 4, 5, 6, 255],
        };
        let encoded = encode_thumbnail_cache(&image).unwrap();
        assert_eq!(decode_thumbnail_cache(&encoded), Some(image));
        assert!(decode_thumbnail_cache(&encoded[..encoded.len() - 1]).is_none());
    }

    #[test]
    fn disk_cache_trimming_enforces_file_and_byte_bounds() {
        let directory = TempDirectory::new("visual-cache-trim");
        for index in 0..5 {
            fs::write(directory.path(&format!("{index}.cache")), vec![0_u8; 16]).unwrap();
        }

        trim_cache(directory.root(), 3, 40).unwrap();

        let files = fs::read_dir(directory.root()).unwrap().count();
        let bytes = fs::read_dir(directory.root())
            .unwrap()
            .map(|entry| entry.unwrap().metadata().unwrap().len())
            .sum::<u64>();
        assert!(files <= 3);
        assert!(bytes <= 40);
    }
}
