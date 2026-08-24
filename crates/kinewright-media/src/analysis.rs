use std::{
    collections::HashSet,
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
        ContentHashes, JsonCache, TempFileStyle, atomic_write, cache_path, cache_root,
        create_cache_dir, read_cache, spawn_worker, trim_cache,
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

pub(crate) struct VisualAssetService {
    jobs: Sender<Job>,
    results: Receiver<VisualAssetResult>,
    in_flight: Arc<Mutex<HashSet<JobKey>>>,
}

impl VisualAssetService {
    pub(crate) fn new(data_dir: &Path) -> Result<Self, MediaError> {
        let (jobs, jobs_rx) = bounded(JOB_CAPACITY);
        let (results_tx, results) = bounded(RESULT_CAPACITY);
        let in_flight = Arc::new(Mutex::new(HashSet::new()));
        let worker_in_flight = Arc::clone(&in_flight);
        let mut worker = VisualAssetWorker::new(
            cache_root(data_dir, "visual-assets", CACHE_VERSION),
            results_tx,
            worker_in_flight,
        );
        spawn_worker(
            "kinewright-visual-assets",
            "visual asset",
            jobs_rx,
            move |job| worker.handle(job),
        )?;
        Ok(Self {
            jobs,
            results,
            in_flight,
        })
    }

    pub(crate) fn request_waveform(&self, asset: MediaAsset) -> bool {
        if !matches!(asset.kind, MediaKind::Audio | MediaKind::AudioVideo) {
            return false;
        }
        self.request(Job::Waveform(asset))
    }

    pub(crate) fn request_thumbnail(
        &self,
        asset: MediaAsset,
        source_at: TimeCode,
        max_width: u32,
    ) -> bool {
        if !matches!(asset.kind, MediaKind::Video | MediaKind::AudioVideo) {
            return false;
        }
        let source_at = TimeCode(
            source_at
                .0
                .clamp(0, asset.duration.0.saturating_sub(1).max(0)),
        );
        self.request(Job::Thumbnail {
            asset,
            source_at,
            max_width: max_width.clamp(1, 512),
        })
    }

    pub(crate) fn results(&self) -> Receiver<VisualAssetResult> {
        self.results.clone()
    }

    fn request(&self, job: Job) -> bool {
        let key = job.key();
        let inserted = self
            .in_flight
            .lock()
            .is_ok_and(|mut in_flight| in_flight.insert(key.clone()));
        if !inserted {
            return true;
        }
        if let Ok(()) = self.jobs.try_send(job) {
            true
        } else {
            if let Ok(mut in_flight) = self.in_flight.lock() {
                in_flight.remove(&key);
            }
            false
        }
    }
}

enum Job {
    Waveform(MediaAsset),
    Thumbnail {
        asset: MediaAsset,
        source_at: TimeCode,
        max_width: u32,
    },
}

impl Job {
    fn key(&self) -> JobKey {
        match self {
            Self::Waveform(asset) => JobKey::Waveform {
                asset: asset.id,
                path: asset.path.clone(),
            },
            Self::Thumbnail {
                asset,
                source_at,
                max_width,
            } => JobKey::Thumbnail {
                key: ThumbnailKey {
                    asset: asset.id,
                    source_at: *source_at,
                    max_width: *max_width,
                },
                path: asset.path.clone(),
            },
        }
    }
}

/// Dedup keys carry the asset's PATH as well as its id: ids are per-document,
/// so with several projects open the same id can name different files, and an
/// id-only key would hand one project's pixels to another's request.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum JobKey {
    Waveform { asset: AssetId, path: PathBuf },
    Thumbnail { key: ThumbnailKey, path: PathBuf },
}

struct VisualAssetWorker {
    root: PathBuf,
    results: Sender<VisualAssetResult>,
    in_flight: Arc<Mutex<HashSet<JobKey>>>,
    hashes: ContentHashes,
}

impl VisualAssetWorker {
    fn new(
        root: PathBuf,
        results: Sender<VisualAssetResult>,
        in_flight: Arc<Mutex<HashSet<JobKey>>>,
    ) -> Self {
        Self {
            root,
            results,
            in_flight,
            hashes: ContentHashes::default(),
        }
    }

    fn handle(&mut self, job: Job) -> bool {
        let key = job.key();
        let result = self.process(job);
        if let Ok(mut in_flight) = self.in_flight.lock() {
            in_flight.remove(&key);
        }
        self.results.send(result).is_ok()
    }

    fn process(&mut self, job: Job) -> VisualAssetResult {
        match job {
            Job::Waveform(asset) => {
                self.waveform(&asset)
                    .unwrap_or_else(|error| VisualAssetResult::Failed {
                        asset: asset.id,
                        path: asset.path.clone(),
                        request: VisualRequestKind::Waveform,
                        message: error.to_string(),
                    })
            }
            Job::Thumbnail {
                asset,
                source_at,
                max_width,
            } => {
                let key = ThumbnailKey {
                    asset: asset.id,
                    source_at,
                    max_width,
                };
                self.thumbnail(&asset, key)
                    .unwrap_or_else(|error| VisualAssetResult::Failed {
                        asset: asset.id,
                        path: asset.path.clone(),
                        request: VisualRequestKind::Thumbnail(key),
                        message: error.to_string(),
                    })
            }
        }
    }

    fn waveform(&mut self, asset: &MediaAsset) -> Result<VisualAssetResult, MediaError> {
        let hash = self.content_hash(&asset.path)?;
        let store = WaveformStore::new(self.root.join("waveforms"));
        if let Some(mut cached) = store.load(&hash, asset.fps, asset.duration)? {
            cached.asset = asset.id;
            cached.path.clone_from(&asset.path);
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
    ) -> Result<VisualAssetResult, MediaError> {
        let hash = self.content_hash(&asset.path)?;
        let store = ThumbnailStore::new(self.root.join("thumbnails"));
        let image = if let Some(cached) = store.load(&hash, key.source_at, key.max_width)? {
            cached
        } else {
            let decoded = thumbnail(&asset.path, asset.fps, key.source_at, key.max_width)?;
            store.save(&hash, key.source_at, key.max_width, &decoded)?;
            decoded
        };
        Ok(VisualAssetResult::Thumbnail(ThumbnailFrame {
            key,
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
        source_at: TimeCode,
        max_width: u32,
    ) -> Result<Option<RgbaImage>, MediaError> {
        let Some(bytes) = read_cache(&self.path_for(hash, source_at, max_width), "thumbnail")?
        else {
            return Ok(None);
        };
        decode_thumbnail_cache(&bytes).map_or(Ok(None), |image| Ok(Some(image)))
    }

    fn save(
        &self,
        hash: &str,
        source_at: TimeCode,
        max_width: u32,
        image: &RgbaImage,
    ) -> Result<(), MediaError> {
        create_cache_dir(&self.root, "thumbnail")?;
        let bytes = encode_thumbnail_cache(image)?;
        atomic_write(
            &self.path_for(hash, source_at, max_width),
            &bytes,
            "visual asset",
            TempFileStyle::ReplaceExtension,
        )?;
        trim_cache(&self.root, MAX_THUMBNAIL_FILES, MAX_THUMBNAIL_BYTES)?;
        Ok(())
    }

    fn path_for(&self, hash: &str, source_at: TimeCode, max_width: u32) -> PathBuf {
        cache_path(
            &self.root,
            hash,
            &format!("-{}-{max_width}", source_at.0),
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
            color_description: kinewright_core::ColorDescription::default(),
        }
    }

    #[test]
    fn a_full_job_queue_is_reported_without_leaking_in_flight_keys() {
        let (jobs, _jobs_rx) = bounded(1);
        let (_results_tx, results) = bounded(1);
        let in_flight = Arc::new(Mutex::new(HashSet::new()));
        let service = VisualAssetService {
            jobs,
            results,
            in_flight: Arc::clone(&in_flight),
        };

        assert!(service.request(Job::Waveform(asset(1))));
        assert!(!service.request(Job::Waveform(asset(2))));
        assert_eq!(in_flight.lock().unwrap().len(), 1);
        assert!(in_flight.lock().unwrap().contains(&JobKey::Waveform {
            asset: AssetId(1),
            path: asset(1).path,
        }));
    }

    #[test]
    fn same_id_different_files_are_distinct_jobs() {
        // Two open projects can both name AssetId(1); the dedup key must
        // treat different files as different work.
        let mut second = asset(1);
        second.path = PathBuf::from("other-project.wav");
        assert_ne!(Job::Waveform(asset(1)).key(), Job::Waveform(second).key());
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
