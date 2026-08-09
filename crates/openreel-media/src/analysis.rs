use std::{
    collections::{HashMap, HashSet},
    fmt::Write as _,
    fs::{self, File},
    io::Read as _,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    thread,
    time::SystemTime,
};

use crossbeam_channel::{Receiver, Sender, bounded};
use openreel_core::{AssetId, MediaAsset, MediaError, MediaKind, Rational, RgbaImage, TimeCode};
use serde::{Deserialize, Serialize};

use crate::{audio::decode_audio_peaks, decode::thumbnail, sha256::Sha256};

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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct WaveformPeak {
    pub minimum: i16,
    pub maximum: i16,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WaveformData {
    pub asset: AssetId,
    pub content_sha256: String,
    pub source_fps: Rational,
    pub source_frames: TimeCode,
    pub peaks: Vec<WaveformPeak>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ThumbnailKey {
    pub asset: AssetId,
    pub source_at: TimeCode,
    pub max_width: u32,
}

#[derive(Debug, Clone)]
pub struct ThumbnailFrame {
    pub key: ThumbnailKey,
    pub image: Arc<RgbaImage>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VisualRequestKind {
    Waveform,
    Thumbnail(ThumbnailKey),
}

#[derive(Debug, Clone)]
pub enum VisualAssetResult {
    Waveform(Arc<WaveformData>),
    Thumbnail(ThumbnailFrame),
    Failed {
        asset: AssetId,
        request: VisualRequestKind,
        message: String,
    },
}

pub(crate) struct VisualAssetService {
    jobs: Sender<Job>,
    results: Receiver<VisualAssetResult>,
    in_flight: Arc<Mutex<HashSet<JobKey>>>,
}

impl VisualAssetService {
    pub(crate) fn new(data_dir: PathBuf) -> Result<Self, MediaError> {
        let (jobs, jobs_rx) = bounded(JOB_CAPACITY);
        let (results_tx, results) = bounded(RESULT_CAPACITY);
        let in_flight = Arc::new(Mutex::new(HashSet::new()));
        let worker_in_flight = Arc::clone(&in_flight);
        thread::Builder::new()
            .name("openreel-visual-assets".to_owned())
            .spawn(move || {
                VisualAssetWorker::new(
                    data_dir.join("visual-assets").join("v1"),
                    jobs_rx,
                    results_tx,
                    worker_in_flight,
                )
                .run();
            })
            .map_err(|error| {
                MediaError::Backend(format!("could not start visual asset worker: {error}"))
            })?;
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
            .is_ok_and(|mut in_flight| in_flight.insert(key));
        if !inserted {
            return true;
        }
        match self.jobs.try_send(job) {
            Ok(()) => true,
            Err(_) => {
                if let Ok(mut in_flight) = self.in_flight.lock() {
                    in_flight.remove(&key);
                }
                false
            }
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
            Self::Waveform(asset) => JobKey::Waveform(asset.id),
            Self::Thumbnail {
                asset,
                source_at,
                max_width,
            } => JobKey::Thumbnail(ThumbnailKey {
                asset: asset.id,
                source_at: *source_at,
                max_width: *max_width,
            }),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum JobKey {
    Waveform(AssetId),
    Thumbnail(ThumbnailKey),
}

struct VisualAssetWorker {
    root: PathBuf,
    jobs: Receiver<Job>,
    results: Sender<VisualAssetResult>,
    in_flight: Arc<Mutex<HashSet<JobKey>>>,
    hashes: HashMap<PathBuf, String>,
}

impl VisualAssetWorker {
    fn new(
        root: PathBuf,
        jobs: Receiver<Job>,
        results: Sender<VisualAssetResult>,
        in_flight: Arc<Mutex<HashSet<JobKey>>>,
    ) -> Self {
        Self {
            root,
            jobs,
            results,
            in_flight,
            hashes: HashMap::new(),
        }
    }

    fn run(mut self) {
        while let Ok(job) = self.jobs.recv() {
            let key = job.key();
            let result = self.process(job);
            if let Ok(mut in_flight) = self.in_flight.lock() {
                in_flight.remove(&key);
            }
            if self.results.send(result).is_err() {
                break;
            }
        }
    }

    fn process(&mut self, job: Job) -> VisualAssetResult {
        match job {
            Job::Waveform(asset) => {
                self.waveform(&asset)
                    .unwrap_or_else(|error| VisualAssetResult::Failed {
                        asset: asset.id,
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
            image: Arc::new(image),
        }))
    }

    fn content_hash(&mut self, path: &Path) -> Result<String, MediaError> {
        if let Some(hash) = self.hashes.get(path) {
            return Ok(hash.clone());
        }
        let hash = sha256_file(path)?;
        self.hashes.insert(path.to_path_buf(), hash.clone());
        Ok(hash)
    }
}

struct WaveformStore {
    root: PathBuf,
}

impl WaveformStore {
    fn new(root: PathBuf) -> Self {
        Self { root }
    }

    fn load(
        &self,
        hash: &str,
        source_fps: Rational,
        source_frames: TimeCode,
    ) -> Result<Option<WaveformData>, MediaError> {
        let path = self.path_for(hash);
        let bytes = match fs::read(path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(cache_error("read waveform", error)),
        };
        let cached = match serde_json::from_slice::<WaveformCacheFile>(&bytes) {
            Ok(cached) => cached,
            Err(_) => return Ok(None),
        };
        if cached.version != CACHE_VERSION
            || cached.waveform.content_sha256 != hash
            || cached.waveform.source_fps != source_fps
            || cached.waveform.source_frames != source_frames
            || cached.waveform.peaks.len() > MAX_WAVEFORM_PEAKS
        {
            return Ok(None);
        }
        Ok(Some(cached.waveform))
    }

    fn save(&self, waveform: &WaveformData) -> Result<(), MediaError> {
        fs::create_dir_all(&self.root).map_err(|error| cache_error("create waveform", error))?;
        let bytes = serde_json::to_vec(&WaveformCacheFile {
            version: CACHE_VERSION,
            waveform: waveform.clone(),
        })
        .map_err(|error| {
            MediaError::Backend(format!("could not encode waveform cache: {error}"))
        })?;
        atomic_write(&self.path_for(&waveform.content_sha256), &bytes)?;
        trim_cache(&self.root, MAX_WAVEFORM_FILES, MAX_WAVEFORM_BYTES)?;
        Ok(())
    }

    fn path_for(&self, hash: &str) -> PathBuf {
        self.root.join(format!("{hash}.json"))
    }
}

#[derive(Serialize, Deserialize)]
struct WaveformCacheFile {
    version: u32,
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
        let bytes = match fs::read(self.path_for(hash, source_at, max_width)) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(cache_error("read thumbnail", error)),
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
        fs::create_dir_all(&self.root).map_err(|error| cache_error("create thumbnail", error))?;
        let bytes = encode_thumbnail_cache(image)?;
        atomic_write(&self.path_for(hash, source_at, max_width), &bytes)?;
        trim_cache(&self.root, MAX_THUMBNAIL_FILES, MAX_THUMBNAIL_BYTES)?;
        Ok(())
    }

    fn path_for(&self, hash: &str, source_at: TimeCode, max_width: u32) -> PathBuf {
        self.root
            .join(format!("{hash}-{}-{max_width}.rgba", source_at.0))
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

fn atomic_write(path: &Path, bytes: &[u8]) -> Result<(), MediaError> {
    let temporary = path.with_extension(format!("tmp-{}", std::process::id()));
    fs::write(&temporary, bytes).map_err(|error| cache_error("write visual asset", error))?;
    if path.exists() {
        fs::remove_file(path).map_err(|error| cache_error("replace visual asset", error))?;
    }
    fs::rename(&temporary, path).map_err(|error| cache_error("commit visual asset", error))
}

fn trim_cache(root: &Path, maximum_files: usize, maximum_bytes: u64) -> Result<(), MediaError> {
    let entries = match fs::read_dir(root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(cache_error("scan visual asset", error)),
    };
    let mut files = entries
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let metadata = entry.metadata().ok()?;
            metadata.is_file().then(|| {
                (
                    entry.path(),
                    metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH),
                    metadata.len(),
                )
            })
        })
        .collect::<Vec<_>>();
    files.sort_by_key(|(_, modified, _)| *modified);
    let mut total_bytes = files.iter().map(|(_, _, bytes)| *bytes).sum::<u64>();
    let mut file_count = files.len();
    for (path, _, bytes) in files {
        if file_count <= maximum_files && total_bytes <= maximum_bytes {
            break;
        }
        if fs::remove_file(path).is_ok() {
            file_count = file_count.saturating_sub(1);
            total_bytes = total_bytes.saturating_sub(bytes);
        }
    }
    Ok(())
}

fn sha256_file(path: &Path) -> Result<String, MediaError> {
    let mut file = File::open(path).map_err(|error| {
        MediaError::Backend(format!("could not hash {}: {error}", path.display()))
    })?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0_u8; 1024 * 1024];
    loop {
        let count = file.read(&mut buffer).map_err(|error| {
            MediaError::Backend(format!("could not hash {}: {error}", path.display()))
        })?;
        if count == 0 {
            break;
        }
        hasher.update(&buffer[..count]);
    }
    let mut encoded = String::with_capacity(64);
    for byte in hasher.finalize() {
        let _ = write!(encoded, "{byte:02x}");
    }
    Ok(encoded)
}

fn cache_error(action: &str, error: std::io::Error) -> MediaError {
    MediaError::Backend(format!("could not {action} cache: {error}"))
}

#[cfg(test)]
mod tests {
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::*;

    struct TempDirectory(PathBuf);

    impl TempDirectory {
        fn new(label: &str) -> Self {
            let nonce = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let path = std::env::temp_dir()
                .join(format!("openreel-{label}-{}-{nonce}", std::process::id()));
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for TempDirectory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn waveform(hash: &str) -> WaveformData {
        WaveformData {
            asset: AssetId(1),
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
        assert!(
            in_flight
                .lock()
                .unwrap()
                .contains(&JobKey::Waveform(AssetId(1)))
        );
    }

    #[test]
    fn waveform_cache_round_trips_and_rebinds_asset() {
        let directory = TempDirectory::new("waveform-cache");
        let store = WaveformStore::new(directory.0.clone());
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
        let store = WaveformStore::new(directory.0.clone());
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
            fs::write(directory.0.join(format!("{index}.cache")), vec![0_u8; 16]).unwrap();
        }

        trim_cache(&directory.0, 3, 40).unwrap();

        let files = fs::read_dir(&directory.0).unwrap().count();
        let bytes = fs::read_dir(&directory.0)
            .unwrap()
            .map(|entry| entry.unwrap().metadata().unwrap().len())
            .sum::<u64>();
        assert!(files <= 3);
        assert!(bytes <= 40);
    }
}
